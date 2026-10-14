use crate::domain::RunKey;
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::{path::Path, time::Duration};

pub struct State {
    pub(crate) db: Connection,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Enqueue {
    Inserted,
    Duplicate,
    Excluded,
}
#[derive(Debug)]
pub struct Claim {
    pub key: RunKey,
    pub project: String,
    pub cwd: String,
    pub token: i64,
    pub attempts: i64,
}
#[derive(Debug, Serialize)]
pub struct RunStatus {
    pub key: RunKey,
    pub project: String,
    pub state: String,
    pub outcome: Option<String>,
    pub reason: Option<String>,
    pub evidence: Option<String>,
}

impl State {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL);
            INSERT INTO schema_version SELECT 1 WHERE NOT EXISTS(SELECT 1 FROM schema_version);",
        )?;
        let version: i64 = db.query_row("SELECT version FROM schema_version", [], |r| r.get(0))?;
        ensure!(version == 1, "Unsupported state schema {version}");
        db.execute_batch("CREATE TABLE IF NOT EXISTS projects(id TEXT PRIMARY KEY, repository TEXT NOT NULL UNIQUE, enabled_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS owned_stores(id TEXT PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS pending_notes(note_id TEXT PRIMARY KEY, reason TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS project_policy(project TEXT PRIMARY KEY REFERENCES projects(id), allow_external INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS extraction_jobs(store TEXT,thread TEXT,turn TEXT,project TEXT NOT NULL REFERENCES projects(id),evidence TEXT NOT NULL,state TEXT NOT NULL DEFAULT 'queued',reason TEXT,available_at INTEGER NOT NULL,lease_until INTEGER,token INTEGER NOT NULL DEFAULT 0,attempts INTEGER NOT NULL DEFAULT 0,provenance TEXT,finished_at INTEGER,PRIMARY KEY(store,thread,turn),FOREIGN KEY(store,thread,turn) REFERENCES runs(store,thread,turn));
            CREATE TABLE IF NOT EXISTS candidates(id TEXT PRIMARY KEY,store TEXT,thread TEXT,turn TEXT,project TEXT NOT NULL,state TEXT NOT NULL,candidate TEXT NOT NULL,created_at INTEGER NOT NULL,FOREIGN KEY(store,thread,turn) REFERENCES extraction_jobs(store,thread,turn));
            CREATE TABLE IF NOT EXISTS exclusions(store TEXT, thread TEXT, turn TEXT, reason TEXT NOT NULL, PRIMARY KEY(store,thread,turn));
            CREATE TABLE IF NOT EXISTS runs(store TEXT, thread TEXT, turn TEXT, project TEXT NOT NULL REFERENCES projects(id), cwd TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'queued', outcome TEXT, reason TEXT, evidence TEXT, available_at INTEGER NOT NULL,
                lease_until INTEGER, token INTEGER NOT NULL DEFAULT 0, attempts INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(store,thread,turn));")?;
        Ok(Self { db })
    }
    pub fn register_project(&mut self, id: &str, repository: &str, enabled: i64) -> Result<()> {
        self.db.execute(
            "INSERT INTO projects VALUES(?1,?2,?3) ON CONFLICT(id) DO NOTHING",
            params![id, repository, enabled],
        )?;
        let stored: String =
            self.db
                .query_row("SELECT repository FROM projects WHERE id=?1", [id], |r| {
                    r.get(0)
                })?;
        ensure!(
            stored == repository,
            "Project identity cannot be reassigned"
        );
        Ok(())
    }
    pub fn project_epoch(&self, id: &str) -> Result<i64> {
        Ok(self
            .db
            .query_row("SELECT enabled_at FROM projects WHERE id=?1", [id], |r| {
                r.get(0)
            })?)
    }
    pub fn own_store(&mut self, id: &str) -> Result<()> {
        self.db
            .execute("INSERT OR IGNORE INTO owned_stores VALUES(?1)", [id])?;
        Ok(())
    }
    pub fn exclude(&mut self, key: &RunKey, reason: &str) -> Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO exclusions VALUES(?1,?2,?3,?4)",
            params![key.store, key.thread, key.turn, reason],
        )?;
        Ok(())
    }
    pub fn enqueue(&mut self, key: &RunKey, project: &str, cwd: &str, now: i64) -> Result<Enqueue> {
        let tx = self.db.transaction()?;
        let excluded: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM owned_stores WHERE id=?1) OR EXISTS(SELECT 1 FROM exclusions WHERE store=?1 AND thread=?2 AND turn=?3)",params![key.store,key.thread,key.turn],|r|r.get(0))?;
        if excluded {
            return Ok(Enqueue::Excluded);
        }
        let inserted=tx.execute("INSERT OR IGNORE INTO runs(store,thread,turn,project,cwd,available_at) VALUES(?1,?2,?3,?4,?5,?6)",params![key.store,key.thread,key.turn,project,cwd,now])?;
        let p: String = tx.query_row(
            "SELECT project FROM runs WHERE store=?1 AND thread=?2 AND turn=?3",
            params![key.store, key.thread, key.turn],
            |r| r.get(0),
        )?;
        ensure!(p == project, "Conflicting run project association");
        tx.commit()?;
        Ok(if inserted == 1 {
            Enqueue::Inserted
        } else {
            Enqueue::Duplicate
        })
    }
    pub fn claim(&mut self, now: i64, lease: i64) -> Result<Option<Claim>> {
        self.claim_store(None, now, lease)
    }
    pub fn claim_store(
        &mut self,
        store: Option<&str>,
        now: i64,
        lease: i64,
    ) -> Result<Option<Claim>> {
        ensure!(lease > 0, "Lease must be positive");
        let until = now
            .checked_add(lease)
            .ok_or_else(|| anyhow::anyhow!("Lease overflow"))?;
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("UPDATE runs SET state='held',reason='excluded' WHERE state IN ('queued','capturing') AND (EXISTS(SELECT 1 FROM owned_stores o WHERE o.id=runs.store) OR EXISTS(SELECT 1 FROM exclusions e WHERE e.store=runs.store AND e.thread=runs.thread AND e.turn=runs.turn))",[])?;
        let claim=tx.query_row("SELECT store,thread,turn,project,cwd,token+1,attempts+1 FROM runs WHERE ((state='queued' AND available_at<=?1) OR (state='capturing' AND lease_until<=?1)) AND (?2 IS NULL OR store=?2) ORDER BY available_at,store,thread,turn LIMIT 1",params![now,store],|r|Ok(Claim {key:RunKey{store:r.get(0)?,thread:r.get(1)?,turn:r.get(2)?},project:r.get(3)?,cwd:r.get(4)?,token:r.get(5)?,attempts:r.get(6)?})).optional()?;
        if let Some(c) = &claim {
            tx.execute("UPDATE runs SET state='capturing',lease_until=?4,token=?5,attempts=attempts+1 WHERE store=?1 AND thread=?2 AND turn=?3",params![c.key.store,c.key.thread,c.key.turn,until,c.token])?;
        }
        tx.commit()?;
        Ok(claim)
    }
    pub fn finish(
        &mut self,
        c: &Claim,
        outcome: &str,
        evidence: Option<&str>,
        now: i64,
    ) -> Result<()> {
        ensure!(
            ["completed", "failed", "interrupted"].contains(&outcome),
            "Unknown terminal outcome"
        );
        ensure!(
            outcome != "completed" || evidence.is_some(),
            "Completed capture requires evidence"
        );
        let state = if outcome == "completed" {
            "ready"
        } else {
            "recorded"
        };
        self.transition(c, state, None, evidence, Some(outcome), now, now)
    }
    pub fn defer(&mut self, c: &Claim, reason: &str, now: i64, delay: i64) -> Result<()> {
        ensure!(delay > 0, "Retry delay must be positive");
        self.transition(
            c,
            "queued",
            Some(reason),
            None,
            None,
            now,
            now.checked_add(delay)
                .ok_or_else(|| anyhow::anyhow!("Delay overflow"))?,
        )
    }
    pub fn hold(&mut self, c: &Claim, reason: &str, now: i64) -> Result<()> {
        self.transition(c, "held", Some(reason), None, None, now, now)
    }
    #[allow(clippy::too_many_arguments)]
    fn transition(
        &mut self,
        c: &Claim,
        state: &str,
        reason: Option<&str>,
        evidence: Option<&str>,
        outcome: Option<&str>,
        now: i64,
        available: i64,
    ) -> Result<()> {
        let n=self.db.execute("UPDATE runs SET state=?4,reason=?5,evidence=?6,outcome=?7,lease_until=NULL,available_at=?8 WHERE store=?1 AND thread=?2 AND turn=?3 AND state='capturing' AND token=?9 AND lease_until>?10 AND NOT EXISTS(SELECT 1 FROM owned_stores WHERE id=?1) AND NOT EXISTS(SELECT 1 FROM exclusions WHERE store=?1 AND thread=?2 AND turn=?3)",params![c.key.store,c.key.thread,c.key.turn,state,reason,evidence,outcome,available,c.token,now])?;
        ensure!(n == 1, "Stale or expired capture claim");
        Ok(())
    }
    pub fn retry(&mut self, key: &RunKey, now: i64) -> Result<()> {
        let n=self.db.execute("UPDATE runs SET state='queued',reason=NULL,attempts=0,available_at=?4 WHERE store=?1 AND thread=?2 AND turn=?3 AND state='held'",params![key.store,key.thread,key.turn,now])?;
        ensure!(n == 1, "Only held runs can be retried");
        Ok(())
    }
    pub fn status(&self) -> Result<Vec<RunStatus>> {
        let mut stmt=self.db.prepare("SELECT store,thread,turn,project,state,outcome,reason,evidence FROM runs ORDER BY available_at")?;
        Ok(stmt
            .query_map([], |r| {
                Ok(RunStatus {
                    key: RunKey {
                        store: r.get(0)?,
                        thread: r.get(1)?,
                        turn: r.get(2)?,
                    },
                    project: r.get(3)?,
                    state: r.get(4)?,
                    outcome: r.get(5)?,
                    reason: r.get(6)?,
                    evidence: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    /// Publication must establish this barrier before changing a note file.
    pub fn mark_note_pending(&mut self, id: &str, reason: &str) -> Result<()> {
        crate::notes::identity(id)?;
        ensure!(
            !reason.is_empty() && reason.len() <= 256,
            "Invalid pending-note reason"
        );
        self.db.execute("INSERT INTO pending_notes VALUES(?1,?2) ON CONFLICT(note_id) DO UPDATE SET reason=excluded.reason",params![id,reason])?;
        Ok(())
    }
    /// Clear only after application finalization and search refresh are complete.
    pub fn clear_note_pending(&mut self, id: &str) -> Result<()> {
        self.db
            .execute("DELETE FROM pending_notes WHERE note_id=?1", [id])?;
        Ok(())
    }
}
