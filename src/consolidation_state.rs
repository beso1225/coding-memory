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
}
impl State {
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
