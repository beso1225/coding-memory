use crate::{
    domain::{RunKey, hash},
    extraction::{Candidate, Validated},
    state::State,
};
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug)]
pub struct ExtractionClaim {
    pub key: RunKey,
    pub project: String,
    pub evidence: String,
    pub token: i64,
    pub attempts: i64,
}
#[derive(Debug, Serialize)]
pub struct ExtractionStatus {
    pub key: RunKey,
    pub state: String,
    pub reason: Option<String>,
    pub attempts: i64,
    pub provenance: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct StagedCandidate {
    pub id: String,
    pub state: String,
    pub candidate: Candidate,
    pub created_at: i64,
}
impl State {
    pub fn external_allowed(&self, project: &str) -> Result<bool> {
        self.project_epoch(project)?;
        Ok(self
            .db
            .query_row(
                "SELECT allow_external FROM project_policy WHERE project=?1",
                [project],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }
    pub fn set_external_policy(&mut self, project: &str, allow: bool) -> Result<()> {
        self.project_epoch(project)?;
        let tx = self.db.transaction()?;
        tx.execute("INSERT INTO project_policy VALUES(?1,?2) ON CONFLICT(project) DO UPDATE SET allow_external=excluded.allow_external",params![project,allow])?;
        if allow {
            tx.execute("UPDATE extraction_jobs SET state='queued',reason=NULL,attempts=0,available_at=?2 WHERE project=?1 AND state='held' AND reason='held_external_policy'",params![project,crate::ingestion::now()])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn enqueue_extractions(&mut self, project: &str, now: i64) -> Result<usize> {
        self.project_epoch(project)?;
        Ok(self.db.execute("INSERT OR IGNORE INTO extraction_jobs(store,thread,turn,project,evidence,available_at) SELECT store,thread,turn,project,evidence,?2 FROM runs r WHERE project=?1 AND state='ready' AND outcome='completed' AND evidence IS NOT NULL AND NOT EXISTS(SELECT 1 FROM owned_stores WHERE id=r.store) AND NOT EXISTS(SELECT 1 FROM exclusions e WHERE e.store=r.store AND e.thread=r.thread AND e.turn=r.turn)",params![project,now])?)
    }
    pub fn claim_extraction(
        &mut self,
        project: &str,
        now: i64,
        lease: i64,
    ) -> Result<Option<ExtractionClaim>> {
        ensure!(lease > 0, "Extraction lease must be positive");
        let until = now
            .checked_add(lease)
            .ok_or_else(|| anyhow::anyhow!("Lease overflow"))?;
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("UPDATE extraction_jobs SET state='held',reason='source_excluded' WHERE state IN ('queued','running') AND (EXISTS(SELECT 1 FROM owned_stores WHERE id=extraction_jobs.store) OR EXISTS(SELECT 1 FROM exclusions e WHERE e.store=extraction_jobs.store AND e.thread=extraction_jobs.thread AND e.turn=extraction_jobs.turn))",[])?;
        tx.execute("UPDATE extraction_jobs SET state='exhausted',reason='generation_lease_expired',lease_until=NULL WHERE project=?1 AND state='running' AND lease_until<=?2 AND attempts>=3",params![project,now])?;
        let claim=tx.query_row("SELECT j.store,j.thread,j.turn,j.project,j.evidence,j.token+1,j.attempts+1 FROM extraction_jobs j JOIN runs r USING(store,thread,turn) WHERE j.project=?1 AND ((j.state='queued' AND j.available_at<=?2) OR(j.state='running' AND j.lease_until<=?2)) AND r.state='ready' AND r.outcome='completed' AND r.evidence=j.evidence ORDER BY j.available_at,j.store,j.thread,j.turn LIMIT 1",params![project,now],|r|Ok(ExtractionClaim{key:RunKey{store:r.get(0)?,thread:r.get(1)?,turn:r.get(2)?},project:r.get(3)?,evidence:r.get(4)?,token:r.get(5)?,attempts:r.get(6)?})).optional()?;
        if let Some(c) = &claim {
            tx.execute("UPDATE extraction_jobs SET state='running',lease_until=?4,token=?5,attempts=attempts+1 WHERE store=?1 AND thread=?2 AND turn=?3",params![c.key.store,c.key.thread,c.key.turn,until,c.token])?;
        }
        tx.commit()?;
        Ok(claim)
    }
    pub fn finish_extraction(
        &mut self,
        c: &ExtractionClaim,
        candidates: &[Validated],
        provenance: &Value,
        now: i64,
    ) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        // The policy decision and candidate insertion share the write transaction.
        let denied = provenance["external_transmission"].as_bool() == Some(true)
            && !tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM project_policy WHERE project=?1 AND allow_external=1)",
                [&c.project],
                |r| r.get::<_, bool>(0),
            )?;
        let n=tx.execute("UPDATE extraction_jobs SET state=?4,reason=NULL,lease_until=NULL,provenance=?5,finished_at=?6 WHERE store=?1 AND thread=?2 AND turn=?3 AND state='running' AND token=?7 AND lease_until>?6 AND NOT EXISTS(SELECT 1 FROM owned_stores WHERE id=?1) AND NOT EXISTS(SELECT 1 FROM exclusions WHERE store=?1 AND thread=?2 AND turn=?3)",params![c.key.store,c.key.thread,c.key.turn,if candidates.is_empty(){"succeeded_empty"}else{"succeeded"},provenance.to_string(),now,c.token])?;
        ensure!(n == 1, "Stale or excluded extraction claim");
        if denied {
            tx.execute("UPDATE extraction_jobs SET state='held',reason='held_external_policy',finished_at=NULL WHERE store=?1 AND thread=?2 AND turn=?3",params![c.key.store,c.key.thread,c.key.turn])?;
            tx.commit()?;
            return Ok(());
        }
        for v in candidates {
            ensure!(
                ["ready", "held"].contains(&v.state.as_str()),
                "Invalid candidate state"
            );
            let id = hash(&serde_json::to_vec(&(&c.key, &v.candidate))?);
            tx.execute(
                "INSERT INTO candidates VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    id,
                    c.key.store,
                    c.key.thread,
                    c.key.turn,
                    c.project,
                    v.state,
                    serde_json::to_string(&v.candidate)?,
                    now
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn fail_extraction(
        &mut self,
        c: &ExtractionClaim,
        reason: &str,
        retryable: bool,
        provenance: &Value,
        now: i64,
    ) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let policy_allowed = reason == "held_external_policy"
            && tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM project_policy WHERE project=?1 AND allow_external=1)",
                [&c.project],
                |r| r.get::<_, bool>(0),
            )?;
        let state = if policy_allowed || (retryable && c.attempts < 3) {
            "queued"
        } else if retryable {
            "exhausted"
        } else {
            "held"
        };
        let available = if policy_allowed {
            now
        } else {
            now.checked_add(10)
                .ok_or_else(|| anyhow::anyhow!("Retry delay overflow"))?
        };
        let n=tx.execute("UPDATE extraction_jobs SET state=?4,reason=?5,lease_until=NULL,available_at=?6,provenance=?7,attempts=CASE WHEN ?10 THEN 0 ELSE attempts END WHERE store=?1 AND thread=?2 AND turn=?3 AND state='running' AND token=?8 AND lease_until>?9",params![c.key.store,c.key.thread,c.key.turn,state,if policy_allowed {None}else{Some(reason)},available,provenance.to_string(),c.token,now,policy_allowed])?;
        ensure!(n == 1, "Stale extraction claim");
        tx.commit()?;
        Ok(())
    }
    pub fn retry_extraction(&mut self, key: &RunKey, now: i64) -> Result<()> {
        let n=self.db.execute("UPDATE extraction_jobs SET state='queued',reason=NULL,attempts=0,available_at=?4 WHERE store=?1 AND thread=?2 AND turn=?3 AND state IN ('held','exhausted')",params![key.store,key.thread,key.turn,now])?;
        ensure!(n == 1, "Only held/exhausted extraction jobs can be retried");
        Ok(())
    }
    pub fn extraction_status(&self, project: &str) -> Result<Vec<ExtractionStatus>> {
        let mut stmt=self.db.prepare("SELECT store,thread,turn,state,reason,attempts,provenance FROM extraction_jobs WHERE project=?1 ORDER BY available_at")?;
        Ok(stmt
            .query_map([project], |r| {
                Ok(ExtractionStatus {
                    key: RunKey {
                        store: r.get(0)?,
                        thread: r.get(1)?,
                        turn: r.get(2)?,
                    },
                    state: r.get(3)?,
                    reason: r.get(4)?,
                    attempts: r.get(5)?,
                    provenance: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn candidates(&self, project: &str) -> Result<Vec<StagedCandidate>> {
        let mut stmt=self.db.prepare("SELECT id,state,candidate,created_at FROM candidates WHERE project=?1 ORDER BY created_at,id")?;
        let rows = stmt
            .query_map([project], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, state, candidate, created_at)| {
                Ok(StagedCandidate {
                    id,
                    state,
                    candidate: serde_json::from_str(&candidate)?,
                    created_at,
                })
            })
            .collect()
    }
}
