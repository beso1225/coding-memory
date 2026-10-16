//! Durable batch reservation and recoverable per-file publication records.
use crate::{extraction_state::StagedCandidate, state::State};
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Debug, Serialize, Deserialize)]
pub struct Batch {
    pub id: i64,
    pub project: String,
    pub candidates: Vec<StagedCandidate>,
}
#[derive(Clone, Copy)]
pub struct Trigger {
    pub force: bool,
    pub min_ready: i64,
    pub max_age_seconds: i64,
}
impl From<bool> for Trigger {
    fn from(force: bool) -> Self {
        Self {
            force,
            min_ready: 5,
            max_age_seconds: 600,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    pub note_id: String,
    pub path: String,
    pub before: Option<String>,
    pub after: String,
    pub content: String,
}
#[derive(Debug, Serialize)]
pub struct BatchStatus {
    pub id: i64,
    pub project: String,
    pub state: String,
    pub reason: Option<String>,
    pub review: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<crate::consolidation_preflight::PartitionDiagnostic>,
}
impl State {
    pub(crate) fn next_batch_id(&self) -> Result<i64> {
        let previous: i64 = self.db.query_row(
            "SELECT coalesce((SELECT seq FROM sqlite_sequence WHERE name='consolidation_jobs'),0)",
            [],
            |r| r.get(0),
        )?;
        previous
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Batch ID overflow"))
    }
    pub(crate) fn preflight_batch(&self, project: &str, id: i64) -> Result<Batch> {
        let valid:bool=self.db.query_row("SELECT EXISTS(SELECT 1 FROM consolidation_jobs WHERE id=?1 AND project=?2 AND state IN ('held','exhausted'))",params![id,project],|r|r.get(0))?;
        ensure!(
            valid,
            "Preflight requires a held or exhausted batch in the requested project"
        );
        let mut q = self.db.prepare("SELECT c.id,c.state,c.candidate,c.created_at,EXISTS(SELECT 1 FROM owned_stores o WHERE o.id=c.store) OR EXISTS(SELECT 1 FROM exclusions e WHERE e.store=c.store AND e.thread=c.thread AND e.turn=c.turn) FROM candidates c JOIN batch_candidates b ON b.candidate=c.id WHERE b.batch=?1 AND c.project=?2 ORDER BY c.created_at,c.id LIMIT 21")?;
        let rows = q
            .query_map(params![id, project], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, bool>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ensure!(
            !rows.is_empty() && rows.len() <= 20,
            "Preflight batch must contain 1..20 candidates"
        );
        let candidates = rows
            .into_iter()
            .map(|(id, state, json, created_at, excluded)| {
                ensure!(
                    state == "held" && !excluded,
                    "Batch contains non-held or excluded candidates"
                );
                Ok(StagedCandidate {
                    id,
                    state,
                    candidate: serde_json::from_str(&json)?,
                    created_at,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Batch {
            id,
            project: project.into(),
            candidates,
        })
    }
    pub fn reserve_batch(
        &mut self,
        project: &str,
        now: i64,
        trigger: impl Into<Trigger>,
    ) -> Result<Option<Batch>> {
        let trigger = trigger.into();
        ensure!(
            (1..=20).contains(&trigger.min_ready) && (1..=86400).contains(&trigger.max_age_seconds),
            "Invalid consolidation trigger"
        );
        self.project_epoch(project)?;
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("UPDATE candidates SET state='held' WHERE state='ready' AND (EXISTS(SELECT 1 FROM owned_stores WHERE id=candidates.store) OR EXISTS(SELECT 1 FROM exclusions e WHERE e.store=candidates.store AND e.thread=candidates.thread AND e.turn=candidates.turn))",[])?;
        let busy:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM consolidation_jobs WHERE project=?1 AND state IN ('generating','applying','conflict'))",[project],|r|r.get(0))?;
        if busy {
            tx.commit()?;
            return Ok(None);
        }
        let (count, oldest): (i64, Option<i64>) = tx.query_row(
            "SELECT count(*),min(created_at) FROM candidates WHERE project=?1 AND state='ready'",
            [project],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if count == 0
            || (!trigger.force
                && count < trigger.min_ready
                && oldest.is_none_or(|t| now.saturating_sub(t) < trigger.max_age_seconds))
        {
            tx.commit()?;
            return Ok(None);
        }
        let rows = {
            let mut stmt=tx.prepare("SELECT id,state,candidate,created_at FROM candidates WHERE project=?1 AND state='ready' ORDER BY created_at,id LIMIT 20")?;
            stmt.query_map([project], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let candidates = rows
            .into_iter()
            .map(|(id, state, json, created_at)| {
                Ok(StagedCandidate {
                    id,
                    state,
                    candidate: serde_json::from_str(&json)?,
                    created_at,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        tx.execute("INSERT INTO consolidation_jobs(project,state,created_at,lease_until) VALUES(?1,'generating',?2,?3)",params![project,now,now.checked_add(3600).ok_or_else(||anyhow::anyhow!("Lease overflow"))?])?;
        let id = tx.last_insert_rowid();
        for c in &candidates {
            tx.execute(
                "INSERT INTO batch_candidates VALUES(?1,?2)",
                params![id, c.id],
            )?;
            tx.execute(
                "UPDATE candidates SET state='reserved' WHERE id=?1",
                [&c.id],
            )?;
        }
        if self.worker_owner.is_some() {
            tx.execute("INSERT OR REPLACE INTO worker_claims VALUES('consolidation',?1,NULL,NULL,NULL,NULL,?2)",params![id.to_string(),id])?;
        }
        tx.commit()?;
        Ok(Some(Batch {
            id,
            project: project.into(),
            candidates,
        }))
    }
    /// Keep only the candidates that fit this generation. Size-only deferrals
    /// lose their reservation link, so they do not count as generation attempts.
    pub(crate) fn partition_batch(
        &mut self,
        batch: &Batch,
        selected: &[String],
        oversized: &[String],
        provenance: &Value,
        now: i64,
    ) -> Result<()> {
        use std::collections::HashSet;
        let original: HashSet<_> = batch.candidates.iter().map(|c| c.id.as_str()).collect();
        let keep: HashSet<_> = selected.iter().map(String::as_str).collect();
        let held: HashSet<_> = oversized.iter().map(String::as_str).collect();
        ensure!(
            original.len() == batch.candidates.len()
                && keep.len() == selected.len()
                && held.len() == oversized.len()
                && keep.is_subset(&original)
                && held.is_subset(&original)
                && keep.is_disjoint(&held),
            "Invalid size partition"
        );
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        ensure!(tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM consolidation_jobs WHERE id=?1 AND project=?2 AND state='generating' AND lease_until>?3)",
            params![batch.id,batch.project,now], |r| r.get::<_,bool>(0)
        )?, "Stale batch");
        let reserved = {
            let mut q = tx.prepare("SELECT c.id,c.state FROM batch_candidates b JOIN candidates c ON c.id=b.candidate WHERE b.batch=?1")?;
            q.query_map([batch.id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        ensure!(
            reserved.len() == original.len()
                && reserved
                    .iter()
                    .all(|(id, state)| original.contains(id.as_str()) && state == "reserved"),
            "Reservation changed during sizing"
        );
        // If no model call fits, reuse the parent job for the first individual
        // hold so the returned batch ID still identifies actionable held work.
        let parent_hold = if selected.is_empty() {
            oversized.first()
        } else {
            None
        };
        for c in &batch.candidates {
            if keep.contains(c.id.as_str()) {
                continue;
            }
            if parent_hold == Some(&c.id) {
                tx.execute("UPDATE candidates SET state='held' WHERE id=?1", [&c.id])?;
                continue;
            }
            tx.execute(
                "DELETE FROM batch_candidates WHERE batch=?1 AND candidate=?2",
                params![batch.id, c.id],
            )?;
            if held.contains(c.id.as_str()) {
                let sizing = provenance
                    .get("diagnostics")
                    .and_then(|v| {
                        serde_json::from_value::<
                                crate::consolidation_preflight::PartitionDiagnostic,
                            >(v.clone())
                            .ok()
                    })
                    .and_then(|d| d.held_candidate(&c.id));
                let diagnostic = serde_json::json!({"schema_version":1,"size_partition_parent":batch.id,"diagnostics":sizing});
                tx.execute("INSERT INTO consolidation_jobs(project,state,reason,created_at,lease_until,provenance) VALUES(?1,'held','held_context_too_large',?2,?2,?3)",params![batch.project,now,diagnostic.to_string()])?;
                let id = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO batch_candidates VALUES(?1,?2)",
                    params![id, c.id],
                )?;
                tx.execute("UPDATE candidates SET state='held' WHERE id=?1", [&c.id])?;
            } else {
                tx.execute("UPDATE candidates SET state='ready' WHERE id=?1", [&c.id])?;
            }
        }
        let mut stored = provenance.clone();
        if let Some(id) = parent_hold {
            let sizing = provenance
                .get("diagnostics")
                .and_then(|v| {
                    serde_json::from_value::<crate::consolidation_preflight::PartitionDiagnostic>(
                        v.clone(),
                    )
                    .ok()
                })
                .and_then(|d| d.held_candidate(id));
            stored["diagnostics"] = serde_json::to_value(sizing)?;
        }
        tx.execute(
            "UPDATE consolidation_jobs SET provenance=?2 WHERE id=?1",
            params![batch.id, stored.to_string()],
        )?;
        if selected.is_empty() {
            if parent_hold.is_some() {
                tx.execute("UPDATE consolidation_jobs SET state='held',reason='held_context_too_large' WHERE id=?1",[batch.id])?;
            } else {
                tx.execute("UPDATE consolidation_jobs SET state='abandoned',reason='size_partitioned' WHERE id=?1",[batch.id])?;
            }
            tx.execute(
                "DELETE FROM worker_claims WHERE stage='consolidation' AND batch=?1",
                [batch.id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn hold_batch(&mut self, batch: i64, reason: &str, provenance: &Value) -> Result<()> {
        self.hold_batch_at(batch, reason, provenance, crate::ingestion::now())
    }
    pub fn hold_batch_at(
        &mut self,
        batch: i64,
        reason: &str,
        provenance: &Value,
        now: i64,
    ) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        ensure!(tx.execute("UPDATE consolidation_jobs SET state='held',reason=?2,provenance=?3 WHERE id=?1 AND state='generating'",params![batch,reason,provenance.to_string()])?==1,"Batch no longer generating");
        tx.execute("UPDATE candidates SET state='held' WHERE id IN (SELECT candidate FROM batch_candidates WHERE batch=?1)",[batch])?;
        tx.execute(
            "INSERT OR REPLACE INTO consolidation_retry VALUES(?1,?2)",
            params![
                batch,
                now.checked_add(10)
                    .ok_or_else(|| anyhow::anyhow!("Retry deadline overflow"))?
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn save_plan(
        &mut self,
        batch: &Batch,
        vault: &std::path::Path,
        changes: &[Change],
        consumed: &[String],
        provenance: &Value,
        now: i64,
    ) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        ensure!(!tx.query_row("SELECT EXISTS(SELECT 1 FROM batch_candidates b JOIN candidates c ON c.id=b.candidate WHERE b.batch=?1 AND (EXISTS(SELECT 1 FROM owned_stores WHERE id=c.store) OR EXISTS(SELECT 1 FROM exclusions e WHERE e.store=c.store AND e.thread=c.thread AND e.turn=c.turn)))",[batch.id],|r|r.get::<_,bool>(0))?,"Source was excluded during generation");
        if provenance["external_transmission"].as_bool() == Some(true) {
            ensure!(tx.query_row("SELECT EXISTS(SELECT 1 FROM project_policy WHERE project=?1 AND allow_external=1)",[&batch.project],|r|r.get::<_,bool>(0))?,"External permission revoked");
        }
        ensure!(tx.execute("UPDATE consolidation_jobs SET state='applying',vault=?2,provenance=?3,consumed=?4 WHERE id=?1 AND state='generating' AND lease_until>?5",params![batch.id,vault.to_str().ok_or_else(||anyhow::anyhow!("Non-UTF8 vault"))?,provenance.to_string(),serde_json::to_string(consumed)?,now])?==1,"Stale batch");
        for change in changes {
            crate::notes::identity(&change.note_id)?;
            ensure!(
                crate::domain::hash(change.content.as_bytes()) == change.after,
                "Change hash mismatch"
            );
            let note = crate::notes::parse(&change.content)?;
            ensure!(
                note.metadata.id == change.note_id,
                "Change identity mismatch"
            );
            tx.execute(
                "INSERT INTO pending_notes VALUES(?1,?2)",
                params![change.note_id, format!("batch-{}", batch.id)],
            )?;
            tx.execute("INSERT INTO vault_changes(batch,note_id,path,before_hash,after_hash,content) VALUES(?1,?2,?3,?4,?5,?6)",params![batch.id,change.note_id,change.path,change.before,change.after,change.content])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn consolidation_status(&self) -> Result<Vec<BatchStatus>> {
        let mut s = self.db.prepare(
            "SELECT id,project,state,reason,provenance FROM consolidation_jobs ORDER BY id",
        )?;
        let rows = s
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, project, state, reason, provenance)| {
                let provenance = provenance
                    .map(|p| serde_json::from_str::<Value>(&p))
                    .transpose()?;
                Ok(BatchStatus {
                    id,
                    project,
                    state,
                    reason,
                    diagnostics: provenance
                        .as_ref()
                        .and_then(|p| p.get("diagnostics"))
                        .and_then(|v| serde_json::from_value(v.clone()).ok()),
                    review: provenance.and_then(|p| p.get("decisions").cloned()),
                })
            })
            .collect()
    }
    pub fn retry_batch(&mut self, id: i64, now: i64) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        ensure!(tx.execute("UPDATE consolidation_jobs SET state='abandoned' WHERE id=?1 AND (state IN ('held','exhausted') OR (state='generating' AND lease_until<=?2))",params![id,now])?==1,"Only held or expired generation batches can retry; application uses recovery");
        tx.execute("UPDATE candidates SET state='ready' WHERE id IN (SELECT candidate FROM batch_candidates WHERE batch=?1) AND state IN ('reserved','held')",[id])?;
        tx.commit()?;
        Ok(())
    }
    pub fn batch_vault(&self, id: i64) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row(
                "SELECT vault FROM consolidation_jobs WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .flatten())
    }
}

#[cfg(test)]
mod partition_tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> Result<(tempfile::TempDir, State, Batch)> {
        let dir = tempfile::tempdir()?;
        let mut state = State::open(&dir.path().join("state.sqlite"))?;
        state.register_project("alpha", "/repo", 1)?;
        let candidate = json!({"kind":"constraint","title":"Use leases","claim":"Use leases.","scope":{"kind":"project","project_id":"alpha"},"conditions":["SQLite"],"verification":"user_stated","evidence":[{"bundle":"0".repeat(64),"item":"U"}]});
        for id in ["a", "b", "c"] {
            state.db.execute("INSERT INTO candidates(id,project,state,candidate,created_at) VALUES(?1,'alpha','ready',?2,1)",params![id,candidate.to_string()])?;
        }
        let batch = state.reserve_batch("alpha", 10, true)?.unwrap();
        Ok((dir, state, batch))
    }
    #[test]
    fn size_partition_rejects_expired_and_replaced_reservations() -> Result<()> {
        let (_dir, mut state, batch) = fixture()?;
        assert!(
            state
                .partition_batch(&batch, &["a".into()], &["b".into()], &json!({}), 3610)
                .is_err()
        );
        assert!(
            state
                .candidates("alpha")?
                .iter()
                .all(|c| c.state == "reserved")
        );
        state.retry_batch(batch.id, 3610)?;
        let successor = state.reserve_batch("alpha", 3611, true)?.unwrap();
        assert!(
            state
                .partition_batch(&batch, &["a".into()], &["b".into()], &json!({}), 3612)
                .is_err()
        );
        assert!(
            state
                .candidates("alpha")?
                .iter()
                .all(|c| c.state == "reserved")
        );
        state.hold_batch_at(successor.id, "fixture", &json!({}), 3613)?;
        Ok(())
    }
    #[test]
    fn partition_write_failure_rolls_back_holds_and_releases_together() -> Result<()> {
        let (_dir, mut state, batch) = fixture()?;
        state.db.execute_batch("CREATE TRIGGER reject_size_release BEFORE UPDATE OF state ON candidates WHEN NEW.state='ready' BEGIN SELECT RAISE(ABORT,'fixture'); END;")?;
        assert!(
            state
                .partition_batch(&batch, &["a".into()], &["b".into()], &json!({}), 11)
                .is_err()
        );
        assert!(
            state
                .candidates("alpha")?
                .iter()
                .all(|c| c.state == "reserved")
        );
        assert_eq!(state.consolidation_status()?.len(), 1);
        assert_eq!(
            state.db.query_row(
                "SELECT count(*) FROM batch_candidates WHERE batch=?1",
                [batch.id],
                |r| r.get::<_, i64>(0)
            )?,
            3
        );
        Ok(())
    }
    #[test]
    fn sizing_holds_do_not_consume_later_model_retry_attempts() -> Result<()> {
        let (_dir, mut state, batch) = fixture()?;
        state.partition_batch(
            &batch,
            &[],
            &["a".into(), "b".into(), "c".into()],
            &json!({}),
            11,
        )?;
        state.retry_batch(batch.id, 12)?;
        for attempt in 0..3 {
            let now = 20 + attempt * 20;
            let batch = state.reserve_batch("alpha", now, true)?.unwrap();
            state.hold_batch_at(batch.id, "generation_timeout", &json!({}), now + 1)?;
            state.retry_transient_batches("alpha", now + 11)?;
            assert_eq!(
                state
                    .candidates("alpha")?
                    .iter()
                    .find(|c| c.id == "a")
                    .unwrap()
                    .state,
                if attempt < 2 { "ready" } else { "held" }
            );
        }
        assert_eq!(
            state.consolidation_status()?.last().unwrap().state,
            "exhausted"
        );
        Ok(())
    }
    #[test]
    fn worker_restart_recovers_only_the_selected_partition() -> Result<()> {
        use crate::worker::Ownership;
        let (dir, mut state, old) = fixture()?;
        state.hold_batch_at(old.id, "fixture", &json!({}), 11)?;
        state.retry_batch(old.id, 12)?;
        let owner = Ownership::acquire(dir.path())?;
        state.begin_worker(&owner, 13)?;
        let batch = state.reserve_batch("alpha", 14, true)?.unwrap();
        state.partition_batch(&batch, &["a".into()], &["b".into()], &json!({}), 15)?;
        drop(state);
        drop(owner);
        let owner = Ownership::acquire(dir.path())?;
        let mut state = State::open(&dir.path().join("state.sqlite"))?;
        state.begin_worker(&owner, 16)?;
        let candidates = state.candidates("alpha")?;
        assert_eq!(
            candidates.iter().find(|c| c.id == "a").unwrap().state,
            "held"
        );
        assert_eq!(
            candidates.iter().find(|c| c.id == "b").unwrap().state,
            "held"
        );
        assert_eq!(
            candidates.iter().find(|c| c.id == "c").unwrap().state,
            "ready"
        );
        state.retry_transient_batches("alpha", 26)?;
        let candidates = state.candidates("alpha")?;
        assert_eq!(
            candidates.iter().find(|c| c.id == "a").unwrap().state,
            "ready"
        );
        assert_eq!(
            candidates.iter().find(|c| c.id == "b").unwrap().state,
            "held"
        );
        assert_eq!(
            candidates.iter().find(|c| c.id == "c").unwrap().state,
            "ready"
        );
        Ok(())
    }
}
