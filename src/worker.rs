//! One local worker: explicit configuration, OS-owned singleton lock and bounded cycles.
use crate::{
    codex::Client,
    consolidation_state::Trigger,
    consolidation_worker, evidence, extraction_worker,
    generation::{Backend, CodexExec},
    ingestion, project,
    state::State,
    vault,
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Duration,
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub project: PathBuf,
    pub codex_home: PathBuf,
    pub codex_binary: PathBuf,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Models {
    pub extraction: String,
    pub consolidation: String,
    pub review: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub vault: PathBuf,
    pub sources: Vec<Source>,
    pub generation_binary: PathBuf,
    pub models: Models,
    #[serde(default = "poll_interval")]
    pub poll_seconds: u64,
    #[serde(default = "generation_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "min_ready")]
    pub min_ready: i64,
    #[serde(default = "max_age")]
    pub max_age_seconds: i64,
    #[serde(default = "system_path")]
    pub executable_path: Vec<PathBuf>,
    /// Synthetic protocol tests only; cloud/local providers remain generation Backends.
    #[serde(default)]
    pub loopback: Option<std::net::SocketAddr>,
}
fn poll_interval() -> u64 {
    30
}
fn generation_timeout() -> u64 {
    300
}
fn min_ready() -> i64 {
    5
}
fn max_age() -> i64 {
    600
}
fn system_path() -> Vec<PathBuf> {
    ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
        .map(PathBuf::from)
        .into()
}
fn absolute_file(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && path.is_file(),
        "Executable/config must be an existing absolute file"
    );
    Ok(())
}
pub(crate) fn executable(path: &Path) -> Result<()> {
    absolute_file(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            path.metadata()?.permissions().mode() & 0o111 != 0,
            "Configured binary is not executable"
        );
    }
    Ok(())
}
impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        absolute_file(path)?;
        Ok(serde_json::from_slice(&evidence::read_bounded(
            path,
            32 * 1024,
        )?)?)
    }
    pub fn validate(&self, root: &Path, state: &State) -> Result<()> {
        ensure!(
            self.schema_version == 1
                && (1..=300).contains(&self.poll_seconds)
                && (1..=600).contains(&self.timeout_seconds)
                && (1..=20).contains(&self.min_ready)
                && (1..=86400).contains(&self.max_age_seconds),
            "Unsupported worker schema or scheduling limits"
        );
        ensure!(
            self.vault.is_absolute()
                && self.vault.is_dir()
                && !root.canonicalize()?.starts_with(self.vault.canonicalize()?),
            "Invalid worker vault"
        );
        executable(&self.generation_binary)?;
        for model in [
            &self.models.extraction,
            &self.models.consolidation,
            &self.models.review,
        ] {
            ensure!(
                !model.trim().is_empty() && model.len() <= 128 && !model.contains('\0'),
                "Invalid worker model"
            );
        }
        ensure!(
            !self.sources.is_empty() && self.sources.len() <= 16,
            "Worker requires 1-16 explicit sources"
        );
        let mut seen = HashSet::new();
        for source in &self.sources {
            ensure!(
                source.project.is_absolute()
                    && source.codex_home.is_absolute()
                    && source.codex_home.is_dir(),
                "Source paths must be existing absolute directories"
            );
            executable(&source.codex_binary)?;
            let p = project::resolve(&source.project)?;
            state.project_epoch(&p.id)?;
            let store = ingestion::store_id(&source.codex_home)?;
            ensure!(
                source.codex_home.canonicalize()?
                    != root.canonicalize()?.join("generation/codex-home"),
                "Generation home cannot be a collection source"
            );
            ensure!(
                !root
                    .canonicalize()?
                    .starts_with(source.project.canonicalize()?),
                "Worker state must be outside collected software repositories"
            );
            ensure!(
                !state.is_owned_store(&store)?,
                "An Engine-owned store cannot be a collection source"
            );
            ensure!(seen.insert((p.id, store)), "Duplicate project/store source");
        }
        ensure!(
            !self.executable_path.is_empty()
                && self.executable_path.len() <= 16
                && self.executable_path.iter().all(|p| p.is_absolute()
                    && p.is_dir()
                    && !p.to_string_lossy().contains([':', '\0'])),
            "Invalid explicit executable search path"
        );
        ensure!(
            self.loopback.is_none_or(|a| a.ip().is_loopback()),
            "Synthetic provider must be loopback"
        );
        Ok(())
    }
    pub fn backend(&self, root: &Path, guard: &Path, model: &str) -> CodexExec {
        CodexExec {
            root: root.into(),
            binary: self.generation_binary.clone(),
            guard_binary: guard.into(),
            model: model.into(),
            timeout: Duration::from_secs(self.timeout_seconds),
            loopback: self.loopback,
        }
    }
}
pub struct Ownership {
    file: std::sync::Arc<LockFile>,
    root: PathBuf,
}
pub(crate) struct LockFile(File);
impl Drop for LockFile {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // A concurrent fork may retain the descriptor until exec. Release the
            // shared lock explicitly when the final parent owner ends its session.
            unsafe {
                libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}
impl Ownership {
    pub fn acquire(root: &Path) -> Result<Self> {
        let path = root.join("worker.lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(path)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            ensure!(
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
                "Another worker owns this state directory"
            );
        }
        #[cfg(not(unix))]
        anyhow::bail!("Worker ownership requires Unix flock");
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(
            serde_json::to_string(
                &json!({"pid":std::process::id(),"started_at":ingestion::now()}),
            )?
            .as_bytes(),
        )?;
        file.sync_all()?;
        Ok(Self {
            file: std::sync::Arc::new(LockFile(file)),
            root: root.canonicalize()?,
        })
    }
}
fn ownership_active(root: &Path) -> Result<bool> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = match options.open(root.join("worker.lock")) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            unsafe {
                libc::flock(file.as_raw_fd(), libc::LOCK_UN);
            };
            return Ok(false);
        }
        let err = std::io::Error::last_os_error();
        ensure!(
            err.kind() == std::io::ErrorKind::WouldBlock,
            "Cannot inspect worker ownership"
        );
        Ok(true)
    }
    #[cfg(not(unix))]
    {
        drop(file);
        Ok(false)
    }
}
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Report {
    pub started_at: i64,
    pub completed_at: Option<i64>,
    pub recovered: usize,
    pub capture_attempts: usize,
    pub extraction_attempts: usize,
    pub consolidation_batches: usize,
    #[serde(default)]
    pub interrupted: bool,
    pub failures: Vec<Failure>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Failure {
    pub project: Option<String>,
    pub stage: String,
}
impl Report {
    fn failed(&mut self, project: Option<&str>, stage: &str) {
        self.failures.push(Failure {
            project: project.map(str::to_owned),
            stage: stage.into(),
        });
    }
}
/// Processing already-captured evidence is independent of whether a reader starts.
pub struct Backends<'a, E: Backend, P: Backend, R: Backend> {
    pub extractor: &'a E,
    pub proposer: &'a P,
    pub reviewer: &'a R,
}
pub async fn process_project<E: Backend, P: Backend, R: Backend>(
    state: &mut State,
    root: &Path,
    config: &Config,
    project: &str,
    backends: Backends<'_, E, P, R>,
    report: &mut Report,
) {
    if state
        .enqueue_extractions(project, ingestion::now())
        .is_err()
    {
        report.failed(Some(project), "extraction_enqueue_failed");
        return;
    }
    for _ in 0..4 {
        match extraction_worker::extract_one(state, root, project, backends.extractor).await {
            Ok(true) => report.extraction_attempts += 1,
            Ok(false) => break,
            Err(_) => {
                report.failed(Some(project), "extraction_failed");
                break;
            }
        }
    }
    match consolidation_worker::consolidate(
        state,
        root,
        &config.vault,
        project,
        Trigger {
            force: false,
            min_ready: config.min_ready,
            max_age_seconds: config.max_age_seconds,
        },
        backends.proposer,
        backends.reviewer,
    )
    .await
    {
        Ok(Some(_)) => report.consolidation_batches += 1,
        Ok(None) => {}
        Err(_) => report.failed(Some(project), "consolidation_failed"),
    }
}
pub async fn cycle(
    state: &mut State,
    root: &Path,
    config: &Config,
    guard: &Path,
) -> Result<Report> {
    let mut report = Report {
        started_at: ingestion::now(),
        ..Report::default()
    };
    state.save_worker_report(&report)?;
    // Only applying batches recover automatically; explicit conflicts require operator intervention.
    for (id, recorded_vault) in state.recoverable_batches()? {
        if recorded_vault != config.vault.canonicalize()?.to_string_lossy() {
            report.failed(None, "recovery_vault_mismatch");
            continue;
        }
        match vault::apply(state, root, &config.vault, id, |_| Ok(())) {
            Ok(true) => report.recovered += 1,
            Ok(false) => report.failed(None, "application_conflict"),
            Err(_) => report.failed(None, "application_recovery_failed"),
        }
    }
    let mut projects = HashSet::new();
    for source in &config.sources {
        let p = match project::resolve(&source.project) {
            Ok(p) => p,
            Err(_) => {
                report.failed(None, "project_unavailable");
                continue;
            }
        };
        if state.project_epoch(&p.id).is_err() {
            report.failed(Some(&p.id), "project_unregistered");
            continue;
        }
        projects.insert(p.id.clone());
        let mut client =
            match Client::open(&source.codex_binary, &source.codex_home, &source.project).await {
                Ok(c) => c,
                Err(_) => {
                    report.failed(Some(&p.id), "reader_start_failed");
                    continue;
                }
            };
        if ingestion::reconcile(state, &mut client, &source.codex_home, &source.project)
            .await
            .is_err()
        {
            report.failed(Some(&p.id), "reconciliation_failed");
        }
        for _ in 0..8 {
            match ingestion::capture_one(state, &mut client, &source.codex_home, root).await {
                Ok(true) => report.capture_attempts += 1,
                Ok(false) => break,
                Err(_) => {
                    report.failed(Some(&p.id), "capture_failed");
                    break;
                }
            }
        }
        if client.close().await.is_err() {
            report.failed(Some(&p.id), "reader_close_failed");
        }
    }
    let extractor = config.backend(root, guard, &config.models.extraction);
    let proposer = config.backend(root, guard, &config.models.consolidation);
    let reviewer = config.backend(root, guard, &config.models.review);
    let mut projects: Vec<_> = projects.into_iter().collect();
    projects.sort();
    for project in projects {
        state.retry_transient_batches(&project, ingestion::now())?;
        process_project(
            state,
            root,
            config,
            &project,
            Backends {
                extractor: &extractor,
                proposer: &proposer,
                reviewer: &reviewer,
            },
            &mut report,
        )
        .await;
    }
    state.db.execute("DELETE FROM worker_claims WHERE (stage='capture' AND NOT EXISTS(SELECT 1 FROM runs r WHERE r.store=worker_claims.store AND r.thread=worker_claims.thread AND r.turn=worker_claims.turn AND r.state='capturing' AND r.token=worker_claims.token)) OR (stage='extraction' AND NOT EXISTS(SELECT 1 FROM extraction_jobs r WHERE r.store=worker_claims.store AND r.thread=worker_claims.thread AND r.turn=worker_claims.turn AND r.state='running' AND r.token=worker_claims.token)) OR (stage='consolidation' AND NOT EXISTS(SELECT 1 FROM consolidation_jobs j WHERE j.id=worker_claims.batch AND j.state='generating'))",[])?;
    report.completed_at = Some(ingestion::now());
    state.save_worker_report(&report)?;
    Ok(report)
}
impl State {
    pub fn is_owned_store(&self, id: &str) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM owned_stores WHERE id=?1)",
            [id],
            |r| r.get(0),
        )?)
    }
    pub fn save_worker_report(&mut self, report: &Report) -> Result<()> {
        self.db.execute("INSERT INTO worker_report VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET report=excluded.report",[serde_json::to_string(report)?])?;
        Ok(())
    }
    pub fn worker_status(&self) -> Result<Value> {
        use rusqlite::OptionalExtension;
        let last: Option<String> = self
            .db
            .query_row("SELECT report FROM worker_report WHERE id=1", [], |r| {
                r.get(0)
            })
            .optional()?;
        let mut counts = serde_json::Map::new();
        for table in [
            "runs",
            "extraction_jobs",
            "candidates",
            "consolidation_jobs",
        ] {
            let mut q = self.db.prepare(&format!(
                "SELECT state,count(*) FROM {table} GROUP BY state"
            ))?;
            let rows = q
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            counts.insert(
                table.into(),
                json!(
                    rows.into_iter()
                        .collect::<std::collections::BTreeMap<_, _>>()
                ),
            );
        }
        let mut problems = serde_json::Map::new();
        for table in ["runs", "extraction_jobs", "consolidation_jobs"] {
            let mut q=self.db.prepare(&format!("SELECT state,reason,count(*) FROM {table} WHERE reason IS NOT NULL GROUP BY state,reason ORDER BY count(*) DESC LIMIT 100"))?;
            let rows=q.query_map([],|r|Ok(json!({"state":r.get::<_,String>(0)?,"reason":r.get::<_,String>(1)?,"count":r.get::<_,i64>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
            problems.insert(table.into(), json!(rows));
        }
        let active = if let Some(dbpath) = self.db.path() {
            ownership_active(
                Path::new(dbpath)
                    .parent()
                    .ok_or_else(|| anyhow::anyhow!("No state parent"))?,
            )?
        } else {
            false
        };
        Ok(
            json!({"active":active,"last_cycle":last.map(|s|serde_json::from_str::<Report>(&s)).transpose()?,"counts":counts,"problems":problems}),
        )
    }
    pub fn recoverable_batches(&self) -> Result<Vec<(i64, String)>> {
        let mut s = self.db.prepare(
            "SELECT id,vault FROM consolidation_jobs WHERE state='applying' ORDER BY id LIMIT 100",
        )?;
        Ok(s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }
}

pub async fn run(
    state: &mut State,
    root: &Path,
    config: &Config,
    guard: &Path,
    once: bool,
) -> Result<()> {
    config.validate(root, state)?;
    let owner = Ownership::acquire(root)?;
    state.begin_worker(&owner, ingestion::now())?;
    let result = run_owned(state, root, config, guard, once).await;
    state.worker_owner = None;
    result
}
async fn run_owned(
    state: &mut State,
    root: &Path,
    config: &Config,
    guard: &Path,
    once: bool,
) -> Result<()> {
    config.validate(root, state)?;

    let shutdown = async {
        #[cfg(unix)]
        {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            tokio::select! {result=tokio::signal::ctrl_c()=>{result?;},_=terminate.recv()=>{}}
        }
        #[cfg(not(unix))]
        tokio::signal::ctrl_c().await?;
        Ok::<_, std::io::Error>(())
    };
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
         result=cycle(state,root,config,guard)=>{let report=result?;println!("{}",serde_json::to_string(&report)?);},
         signal=&mut shutdown=>{signal?;state.interrupt_worker_report()?;return Ok(());}
        }
        if once {
            return Ok(());
        }
        tokio::select! {_=tokio::time::sleep(Duration::from_secs(config.poll_seconds))=>{},signal=&mut shutdown=>{signal?;state.interrupt_worker_report()?;return Ok(());}}
    }
}
impl State {
    fn interrupt_worker_report(&mut self) -> Result<()> {
        let value = self.worker_status()?;
        if !value["last_cycle"].is_null() {
            let mut report: Report = serde_json::from_value(value["last_cycle"].clone())?;
            if report.completed_at.is_none() {
                report.interrupted = true;
                report.completed_at = Some(ingestion::now());
                self.save_worker_report(&report)?;
            }
        }
        Ok(())
    }
}
impl State {
    pub fn begin_worker(&mut self, owner: &Ownership, now: i64) -> Result<()> {
        use rusqlite::params;
        let path = self
            .db
            .path()
            .ok_or_else(|| anyhow::anyhow!("Worker requires on-disk state"))?;
        ensure!(
            Path::new(path)
                .parent()
                .ok_or_else(|| anyhow::anyhow!("Missing state parent"))?
                .canonicalize()?
                == owner.root,
            "Worker lock belongs to another state directory"
        );
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for (stage, table, active) in [
            ("capture", "runs", "capturing"),
            ("extraction", "extraction_jobs", "running"),
        ] {
            let state = if stage == "extraction" {
                "CASE WHEN attempts>=3 THEN 'exhausted' ELSE 'queued' END"
            } else {
                "'queued'"
            };
            tx.execute(&format!("UPDATE {table} SET state={state},reason='worker_interrupted',lease_until=NULL,available_at=?1,token=token+1 WHERE state=?2 AND EXISTS(SELECT 1 FROM worker_claims w WHERE w.stage=?3 AND w.store={table}.store AND w.thread={table}.thread AND w.turn={table}.turn AND w.token={table}.token)"),params![now,active,stage])?;
        }
        tx.execute("UPDATE consolidation_jobs SET state='held',reason='worker_interrupted' WHERE state='generating' AND id IN (SELECT batch FROM worker_claims WHERE stage='consolidation')",[])?;
        tx.execute("UPDATE candidates SET state='held' WHERE state='reserved' AND id IN (SELECT b.candidate FROM batch_candidates b JOIN consolidation_jobs j ON j.id=b.batch WHERE j.state='held' AND j.reason='worker_interrupted')",[])?;
        tx.execute("INSERT OR REPLACE INTO consolidation_retry SELECT id,?1 FROM consolidation_jobs WHERE state='held' AND reason='worker_interrupted' AND id IN (SELECT batch FROM worker_claims WHERE stage='consolidation')",[now.checked_add(10).ok_or_else(||anyhow::anyhow!("Retry deadline overflow"))?])?;
        tx.execute("DELETE FROM worker_claims", [])?;
        tx.commit()?;
        self.worker_owner = Some(owner.file.clone());
        Ok(())
    }
    pub fn retry_transient_batches(&mut self, project: &str, now: i64) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let jobs = {
            let mut q=tx.prepare("SELECT j.id FROM consolidation_jobs j JOIN consolidation_retry r ON r.batch=j.id WHERE j.project=?1 AND j.state='held' AND j.reason IN ('generation_timeout','generation_process_failure','worker_interrupted') AND r.available_at<=?2 ORDER BY j.id LIMIT 100")?;
            q.query_map(rusqlite::params![project, now], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for id in jobs {
            let maximum:i64=tx.query_row("SELECT max((SELECT count(*) FROM batch_candidates old WHERE old.candidate=current.candidate)) FROM batch_candidates current WHERE current.batch=?1",[id],|r|r.get(0))?;
            if maximum >= 3 {
                tx.execute(
                    "UPDATE consolidation_jobs SET state='exhausted' WHERE id=?1",
                    [id],
                )?;
                continue;
            }
            tx.execute(
                "UPDATE consolidation_jobs SET state='abandoned' WHERE id=?1",
                [id],
            )?;
            tx.execute("UPDATE candidates SET state='ready' WHERE state='held' AND id IN (SELECT candidate FROM batch_candidates WHERE batch=?1)",[id])?;
        }
        tx.commit()?;
        Ok(())
    }
}
