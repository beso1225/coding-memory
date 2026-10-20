use crate::clock;
use crate::domain::RunKey;
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::{path::Path, time::Duration};

pub struct State {
    pub(crate) db: Connection,
    pub(crate) worker_owner: Option<std::sync::Arc<crate::worker::LockFile>>,
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
    /// Read existing state without schema setup, journal changes, or directory
    /// creation. The database is write-ahead logged, so a read-only connection may
    /// still create or reuse SQLite's `-wal`/`-shm` coordination files while another
    /// connection holds it open; no schema or rows are written.
    pub fn open_read_only(path: &Path) -> Result<Self> {
        let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        db.busy_timeout(Duration::from_secs(5))?;
        let version: i64 = db.query_row("SELECT version FROM schema_version", [], |r| r.get(0))?;
        ensure!(version == 1, "Unsupported state schema");
        Ok(Self {
            db,
            worker_owner: None,
        })
    }
    pub fn open(path: &Path) -> Result<Self> {
        let mut db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        Self::enable_wal(&db)?;
        db.execute_batch(
            "PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL);
            INSERT INTO schema_version SELECT 1 WHERE NOT EXISTS(SELECT 1 FROM schema_version);",
        )?;
        let version: i64 = db.query_row("SELECT version FROM schema_version", [], |r| r.get(0))?;
        ensure!(version == 1, "Unsupported state schema {version}");
        let projects_exist: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='projects')",
            [],
            |row| row.get(0),
        )?;
        Self::migrate_v1(&mut db, projects_exist)?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS runtime_checks(digest TEXT NOT NULL,fixture TEXT NOT NULL,binary TEXT NOT NULL,version TEXT NOT NULL,state TEXT NOT NULL,last_result TEXT,checked_at INTEGER,available_at INTEGER NOT NULL,lease_until INTEGER,token INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(digest,fixture));
            CREATE TABLE IF NOT EXISTS projects(id TEXT PRIMARY KEY, repository TEXT NOT NULL UNIQUE, enabled_at INTEGER NOT NULL, enabled_ms INTEGER);
            CREATE TABLE IF NOT EXISTS owned_stores(id TEXT PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS pending_notes(note_id TEXT PRIMARY KEY, reason TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS project_policy(project TEXT PRIMARY KEY REFERENCES projects(id), allow_external INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS extraction_jobs(store TEXT,thread TEXT,turn TEXT,project TEXT NOT NULL REFERENCES projects(id),evidence TEXT NOT NULL,state TEXT NOT NULL DEFAULT 'queued',reason TEXT,available_at INTEGER NOT NULL,lease_until INTEGER,token INTEGER NOT NULL DEFAULT 0,attempts INTEGER NOT NULL DEFAULT 0,provenance TEXT,finished_at INTEGER,PRIMARY KEY(store,thread,turn),FOREIGN KEY(store,thread,turn) REFERENCES runs(store,thread,turn));
            CREATE TABLE IF NOT EXISTS candidates(id TEXT PRIMARY KEY,store TEXT,thread TEXT,turn TEXT,project TEXT NOT NULL,state TEXT NOT NULL,candidate TEXT NOT NULL,created_at INTEGER NOT NULL,FOREIGN KEY(store,thread,turn) REFERENCES extraction_jobs(store,thread,turn));
            CREATE TABLE IF NOT EXISTS extraction_progress(store TEXT,thread TEXT,turn TEXT,chunk INTEGER NOT NULL,evidence TEXT NOT NULL,prompt_version TEXT NOT NULL,generator TEXT NOT NULL,plan TEXT NOT NULL,candidates TEXT NOT NULL,PRIMARY KEY(store,thread,turn,chunk),FOREIGN KEY(store,thread,turn) REFERENCES extraction_jobs(store,thread,turn));
            CREATE TABLE IF NOT EXISTS consolidation_retry(batch INTEGER PRIMARY KEY REFERENCES consolidation_jobs(id),available_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS worker_claims(stage TEXT,id TEXT,store TEXT,thread TEXT,turn TEXT,token INTEGER,batch INTEGER,PRIMARY KEY(stage,id));
            CREATE TABLE IF NOT EXISTS worker_report(id INTEGER PRIMARY KEY CHECK(id=1),report TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS evidence_cleanup(digest TEXT PRIMARY KEY,deleted_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS consolidation_jobs(id INTEGER PRIMARY KEY AUTOINCREMENT,project TEXT NOT NULL REFERENCES projects(id),state TEXT NOT NULL,reason TEXT,created_at INTEGER NOT NULL,lease_until INTEGER NOT NULL,vault TEXT,provenance TEXT,consumed TEXT);
            CREATE TABLE IF NOT EXISTS batch_candidates(batch INTEGER REFERENCES consolidation_jobs(id),candidate TEXT REFERENCES candidates(id),PRIMARY KEY(batch,candidate));
            CREATE TABLE IF NOT EXISTS vault_changes(batch INTEGER REFERENCES consolidation_jobs(id),note_id TEXT NOT NULL,path TEXT NOT NULL,before_hash TEXT,after_hash TEXT NOT NULL,content TEXT NOT NULL,applied INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(batch,note_id));
            CREATE TABLE IF NOT EXISTS exclusions(store TEXT, thread TEXT, turn TEXT, reason TEXT NOT NULL, PRIMARY KEY(store,thread,turn));
            CREATE TABLE IF NOT EXISTS capture_hints(store TEXT, thread TEXT, turn TEXT, at INTEGER NOT NULL, PRIMARY KEY(store,thread,turn));
            CREATE TABLE IF NOT EXISTS pending_hints(store TEXT, thread TEXT, turn TEXT, cwd TEXT NOT NULL, at INTEGER NOT NULL, PRIMARY KEY(store,thread,turn));
            CREATE TABLE IF NOT EXISTS runs(store TEXT, thread TEXT, turn TEXT, project TEXT NOT NULL REFERENCES projects(id), cwd TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'queued', outcome TEXT, reason TEXT, evidence TEXT, available_at INTEGER NOT NULL,
                lease_until INTEGER, token INTEGER NOT NULL DEFAULT 0, attempts INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(store,thread,turn));")?;
        Ok(Self {
            db,
            worker_owner: None,
        })
    }
    /// Switch the database to WAL, tolerating a concurrent first open.
    ///
    /// Changing the journal mode needs exclusive access and does not wait on
    /// the busy handler. Overlapping bridge subprocesses opening a
    /// just-created database therefore have to retry the transition; once one
    /// succeeds, the others observe WAL and return immediately.
    fn enable_wal(db: &Connection) -> Result<()> {
        for _ in 0..250 {
            match db.pragma_update(None, "journal_mode", "WAL") {
                Ok(()) => return Ok(()),
                Err(rusqlite::Error::SqliteFailure(error, _))
                    if error.code == rusqlite::ErrorCode::DatabaseBusy =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error.into()),
            }
        }
        anyhow::bail!("Timed out enabling write-ahead logging")
    }

    /// Register a project, sampling the exact instant now.
    ///
    /// A caller that scans a registration baseline must use
    /// [`Self::register_project_at`] instead: the baseline read can take
    /// longer than a second, and the persisted boundary must be the instant
    /// chosen before that scan.
    pub fn register_project(&mut self, id: &str, repository: &str, enabled: i64) -> Result<()> {
        self.register_project_at(id, repository, enabled, clock::now_ms())
            .map(|_| ())
    }
    /// Register a project with the exact instant its caller chose.
    ///
    /// The second-resolution epoch stays the public boundary; the exact instant
    /// is recorded beside it wherever a source can express more. Returns whether
    /// this call inserted the project: a concurrent registration that lost the
    /// insert race must not apply a baseline under its own later instant.
    pub fn register_project_at(
        &mut self,
        id: &str,
        repository: &str,
        enabled: i64,
        enabled_ms: i64,
    ) -> Result<bool> {
        ensure!(
            enabled_ms >= enabled.saturating_mul(1000),
            "Registration instant precedes the recorded epoch"
        );
        let inserted = self.db.execute(
            "INSERT INTO projects(id,repository,enabled_at,enabled_ms) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO NOTHING",
            params![id, repository, enabled, enabled_ms],
        )? == 1;
        let stored: String =
            self.db
                .query_row("SELECT repository FROM projects WHERE id=?1", [id], |r| {
                    r.get(0)
                })?;
        ensure!(
            stored == repository,
            "Project identity cannot be reassigned"
        );
        Ok(inserted)
    }
    /// Bring a schema-version-1 database up to the columns this build reads.
    ///
    /// `CREATE TABLE IF NOT EXISTS` never alters an existing table, so a column
    /// added later must be added explicitly: otherwise an upgraded installation
    /// fails when it selects it, and registering a project fails on the insert.
    fn migrate_v1(db: &mut Connection, projects_exist: bool) -> Result<()> {
        // A fresh database has no table yet: the schema below creates it with
        // the column, so there is nothing to migrate.
        if !projects_exist {
            return Ok(());
        }
        // Concurrent bridge subprocesses can open the same database for
        // overlapping turns. An unlocked check-then-alter would let both see
        // the column missing, and the loser would fail with a duplicate-column
        // error before recording its turn hint. An immediate transaction
        // serializes the check and the alteration.
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let columns: Vec<String> = {
            let mut statement = tx.prepare("PRAGMA table_info(projects)")?;
            statement
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<_, _>>()?
        };
        if !columns.iter().any(|column| column == "enabled_ms") {
            // Rows written before this column existed keep no exact instant;
            // readers fall back to their whole-second epoch.
            tx.execute("ALTER TABLE projects ADD COLUMN enabled_ms INTEGER", [])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The exact registration instant in milliseconds, when recorded.
    ///
    /// Enabling a project inside one second would otherwise admit every run that
    /// ended anywhere in that same second, including ones before the instant.
    pub fn project_epoch_ms(&self, id: &str) -> Result<Option<i64>> {
        Ok(self
            .db
            .query_row("SELECT enabled_ms FROM projects WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()?
            .flatten())
    }
    pub fn project_epoch(&self, id: &str) -> Result<i64> {
        Ok(self
            .db
            .query_row("SELECT enabled_at FROM projects WHERE id=?1", [id], |r| {
                r.get(0)
            })?)
    }
    /// The registration epoch when the project is enabled, or `None`.
    ///
    /// Callers must not distinguish "not registered" from a database failure by
    /// inspecting an error from [`Self::project_epoch`], which would also defer
    /// a hint on an unrelated storage fault.
    pub fn project_enabled(&self, id: &str) -> Result<Option<i64>> {
        Ok(self
            .db
            .query_row("SELECT enabled_at FROM projects WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()?)
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
    /// Record that a live runtime observed this turn end, and queue it.
    ///
    /// The hint is what authorizes capture: `claim` refuses a run no hint
    /// introduced, so a run discovered only by reconciliation or retry is never
    /// captured from a session read that never saw this turn end.
    pub fn enqueue_hinted(
        &mut self,
        key: &RunKey,
        project: &str,
        cwd: &str,
        now: i64,
    ) -> Result<Enqueue> {
        // One transaction: a crash between the two writes would leave a queued
        // run that capture permanently holds as `no_capture_hint`.
        self.enqueue_in(key, project, cwd, now, Some(now))
    }
    /// Record a hint for a project that is not registered yet.
    ///
    /// A live run can end before its project is registered; the hint still names
    /// the exact run a process observed ending, so it is kept and resolved by a
    /// later `reconcile` instead of being dropped.
    pub fn save_pending_hint(&mut self, key: &RunKey, cwd: &str, now: i64) -> Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO pending_hints VALUES(?1,?2,?3,?4,?5)",
            params![key.store, key.thread, key.turn, cwd, now],
        )?;
        Ok(())
    }
    /// Pending hints for one runtime store, oldest first.
    pub fn pending_hints(&self, store: &str) -> Result<Vec<(RunKey, String)>> {
        let mut stmt = self.db.prepare(
            "SELECT store,thread,turn,cwd FROM pending_hints WHERE store=?1 ORDER BY at,thread,turn",
        )?;
        let rows = stmt.query_map([store], |r| {
            Ok((
                RunKey {
                    store: r.get(0)?,
                    thread: r.get(1)?,
                    turn: r.get(2)?,
                },
                r.get::<_, String>(3)?,
            ))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
    /// Forget a hinted run whose project is now registered and which was queued.
    pub fn resolve_hint(&mut self, key: &RunKey) -> Result<()> {
        self.db.execute(
            "DELETE FROM pending_hints WHERE store=?1 AND thread=?2 AND turn=?3",
            params![key.store, key.thread, key.turn],
        )?;
        Ok(())
    }
    /// Drop hints captured before `before`, so a turn that never appears in a
    /// projection cannot accumulate forever.
    pub fn expire_pending_hints(&mut self, store: &str, before: i64) -> Result<usize> {
        Ok(self.db.execute(
            "DELETE FROM pending_hints WHERE store=?1 AND at < ?2",
            params![store, before],
        )?)
    }
    /// Whether a live runtime observed this exact turn end.
    pub fn hinted(&self, key: &RunKey) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM capture_hints WHERE store=?1 AND thread=?2 AND turn=?3)",
            params![key.store, key.thread, key.turn],
            |r| r.get(0),
        )?)
    }
    pub fn enqueue(&mut self, key: &RunKey, project: &str, cwd: &str, now: i64) -> Result<Enqueue> {
        // Reconciliation queues work but never authorizes capture: only a live
        // hint does, so this path records no hint.
        self.enqueue_in(key, project, cwd, now, None)
    }
    /// Queue one run, and record its capture hint when `hinted_at` is set, in a
    /// single transaction.
    fn enqueue_in(
        &mut self,
        key: &RunKey,
        project: &str,
        cwd: &str,
        now: i64,
        hinted_at: Option<i64>,
    ) -> Result<Enqueue> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
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
        if let Some(at) = hinted_at {
            tx.execute(
                "INSERT OR IGNORE INTO capture_hints VALUES(?1,?2,?3,?4)",
                params![key.store, key.thread, key.turn, at],
            )?;
        }
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
    /// Claim one session's run, optionally pinned to an exact turn.
    ///
    /// A capture reads one session's projection, so it may only publish that
    /// session's run; claiming the store's oldest run could lease, or complete,
    /// an unrelated session's work. Pinning keeps a `capture --turn N` from
    /// leasing a different queued run of the same session, which would hold it
    /// `capturing` for the lease duration even though this command can never
    /// use it.
    pub fn claim_session_turn(
        &mut self,
        store: Option<&str>,
        thread: &str,
        turn: Option<&str>,
        now: i64,
        lease: i64,
    ) -> Result<Option<Claim>> {
        self.claim_matching(store, Some(thread), turn, now, lease)
    }
    pub fn claim_store(
        &mut self,
        store: Option<&str>,
        now: i64,
        lease: i64,
    ) -> Result<Option<Claim>> {
        self.claim_matching(store, None, None, now, lease)
    }
    /// Claim by store and optional exact `thread` / `turn` filters.
    fn claim_matching(
        &mut self,
        store: Option<&str>,
        thread: Option<&str>,
        turn: Option<&str>,
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
        let claim=tx.query_row("SELECT store,thread,turn,project,cwd,token+1,attempts+1 FROM runs WHERE ((state='queued' AND available_at<=?1) OR (state='capturing' AND lease_until<=?1)) AND (?2 IS NULL OR store=?2) AND (?3 IS NULL OR thread=?3) AND (?4 IS NULL OR turn=?4) ORDER BY available_at,store,thread,turn LIMIT 1",params![now,store,thread,turn],|r|Ok(Claim {key:RunKey{store:r.get(0)?,thread:r.get(1)?,turn:r.get(2)?},project:r.get(3)?,cwd:r.get(4)?,token:r.get(5)?,attempts:r.get(6)?})).optional()?;
        if let Some(c) = &claim {
            tx.execute("UPDATE runs SET state='capturing',lease_until=?4,token=?5,attempts=attempts+1 WHERE store=?1 AND thread=?2 AND turn=?3",params![c.key.store,c.key.thread,c.key.turn,until,c.token])?;
        }
        if self.worker_owner.is_some()
            && let Some(c) = &claim
        {
            tx.execute(
                "INSERT OR REPLACE INTO worker_claims VALUES('capture',?1,?2,?3,?4,?5,NULL)",
                params![c.key.id(), c.key.store, c.key.thread, c.key.turn, c.token],
            )?;
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
    /// Hold a run without claiming it, even while its retry window is open.
    ///
    /// The over-limit bridge request is a terminal decision about a turn whose
    /// projection cannot be transported. Waiting for `available_at` (a pending
    /// capture deferred the same run) would leave it queued with no driver. A
    /// live lease is still respected so another process's transition is not
    /// stolen.
    pub fn hold_run(&mut self, key: &RunKey, reason: &str, now: i64) -> Result<bool> {
        Ok(self.db.execute(
            "UPDATE runs SET state='held',reason=?4,evidence=NULL,outcome=NULL,
                lease_until=NULL,available_at=?5
             WHERE store=?1 AND thread=?2 AND turn=?3
               AND (state='queued' OR (state='capturing' AND lease_until<=?5))",
            params![key.store, key.thread, key.turn, reason, now],
        )? == 1)
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
        Self::transition_on(
            &self.db, c, state, reason, evidence, outcome, now, available,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn transition_on(
        db: &Connection,
        c: &Claim,
        state: &str,
        reason: Option<&str>,
        evidence: Option<&str>,
        outcome: Option<&str>,
        now: i64,
        available: i64,
    ) -> Result<()> {
        let n=db.execute("UPDATE runs SET state=?4,reason=?5,evidence=?6,outcome=?7,lease_until=NULL,available_at=?8 WHERE store=?1 AND thread=?2 AND turn=?3 AND state='capturing' AND token=?9 AND lease_until>?10 AND NOT EXISTS(SELECT 1 FROM owned_stores WHERE id=?1) AND NOT EXISTS(SELECT 1 FROM exclusions WHERE store=?1 AND thread=?2 AND turn=?3)",params![c.key.store,c.key.thread,c.key.turn,state,reason,evidence,outcome,available,c.token,now])?;
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
