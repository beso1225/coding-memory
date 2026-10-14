//! Durable local verification jobs. Unknown/stale binaries may capture provisionally.
use crate::{
    domain::{RuntimeAudit, hash},
    state::State,
};
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::OnceLock,
};
pub const INTERVAL: i64 = 7 * 24 * 60 * 60;
const LEASE: i64 = 120;
pub fn fixture_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        hash(
            &[
                include_bytes!("runtime_probe.rs").as_slice(),
                include_bytes!("codex.rs").as_slice(),
                include_bytes!("compatibility.rs").as_slice(),
                include_bytes!("../Cargo.lock").as_slice(),
            ]
            .concat(),
        )
    })
}
#[derive(Clone, Debug)]
pub struct Identity {
    pub binary: PathBuf,
    pub digest: String,
    pub version: String,
}
pub fn fingerprint(binary: &Path) -> Result<String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let mut file = options.open(binary)?;
    ensure!(file.metadata()?.is_file(), "Not a binary file");
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    let mut total = 0;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n;
        ensure!(
            total <= 512 * 1024 * 1024,
            "Binary exceeds verification size limit"
        );
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
pub fn resolve(binary: &Path) -> Result<PathBuf> {
    if binary.is_absolute() || binary.components().count() > 1 {
        return Ok(binary.canonicalize()?);
    }
    let path = std::env::var_os("PATH").ok_or_else(|| anyhow::anyhow!("No executable PATH"))?;
    for dir in std::env::split_paths(&path) {
        let p = dir.join(binary);
        if crate::worker::executable(&p).is_ok() {
            return Ok(p.canonicalize()?);
        }
    }
    anyhow::bail!("Cannot resolve executable")
}
pub async fn identify(binary: &Path, home: &Path) -> Result<Identity> {
    let binary = resolve(binary)?;
    let version = crate::generation::version(&binary, home)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.reason()))?;
    ensure!(
        crate::codex::supported(&version),
        "Reader below minimum version"
    );
    Ok(Identity {
        digest: fingerprint(&binary)?,
        binary,
        version,
    })
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Job {
    pub binary: String,
    pub digest: String,
    pub fixture: String,
    pub version: String,
    pub token: i64,
}
impl State {
    pub fn observe_runtime(&mut self, identity: &Identity, now: i64) -> Result<RuntimeAudit> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("INSERT OR IGNORE INTO runtime_checks(digest,fixture,binary,version,state,available_at) VALUES(?1,?2,?3,?4,'queued',?5)",params![identity.digest,fixture_id(),identity.binary.to_str().ok_or_else(||anyhow::anyhow!("Non-UTF8 binary"))?,identity.version,now])?;
        tx.execute("UPDATE runtime_checks SET state='queued',available_at=?3 WHERE digest=?1 AND fixture=?2 AND state='complete' AND (checked_at>?3 OR checked_at<=?4)",params![identity.digest,fixture_id(),now,now.saturating_sub(INTERVAL)])?;
        let (result, checked): (Option<String>, Option<i64>) = tx.query_row(
            "SELECT last_result,checked_at FROM runtime_checks WHERE digest=?1 AND fixture=?2",
            params![identity.digest, fixture_id()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let latest: Option<String> = tx.query_row("SELECT last_result FROM runtime_checks WHERE digest=?1 AND last_result IS NOT NULL ORDER BY checked_at DESC,(fixture=?2) DESC LIMIT 1",params![identity.digest,fixture_id()],|r|r.get(0)).optional()?;
        tx.commit()?;
        let status = if result.as_deref() == Some("failed") || latest.as_deref() == Some("failed") {
            "failed"
        } else if result.as_deref() == Some("passed")
            && checked.is_some_and(|t| t <= now && now - t < INTERVAL)
        {
            "verified"
        } else {
            "provisional"
        };
        Ok(RuntimeAudit {
            binary_sha256: identity.digest.clone(),
            reader_version: identity.version.clone(),
            fixture: fixture_id().into(),
            validation: status.into(),
            checked_at: checked,
        })
    }
    pub fn claim_runtime_check(&mut self, now: i64) -> Result<Option<Job>> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("UPDATE runtime_checks SET state='obsolete',lease_until=NULL WHERE fixture!=?1 AND (state='queued' OR (state='running' AND lease_until<=?2))",params![fixture_id(),now])?;
        let job=tx.query_row("SELECT binary,digest,fixture,version,token+1 FROM runtime_checks WHERE (state='queued' AND available_at<=?1) OR (state='running' AND lease_until<=?1) ORDER BY available_at,digest LIMIT 1",[now],|r|Ok(Job{binary:r.get(0)?,digest:r.get(1)?,fixture:r.get(2)?,version:r.get(3)?,token:r.get(4)?})).optional()?;
        if let Some(j) = &job {
            tx.execute("UPDATE runtime_checks SET state='running',token=?3,lease_until=?4 WHERE digest=?1 AND fixture=?2",params![j.digest,j.fixture,j.token,now.checked_add(LEASE).ok_or_else(||anyhow::anyhow!("Verification lease overflow"))?])?;
        }
        tx.commit()?;
        Ok(job)
    }
    pub fn finish_runtime_check(&mut self, job: &Job, passed: bool, now: i64) -> Result<()> {
        ensure!(self.db.execute("UPDATE runtime_checks SET state='complete',last_result=?4,checked_at=?5,lease_until=NULL WHERE digest=?1 AND fixture=?2 AND token=?3 AND state='running' AND lease_until>?5",params![job.digest,job.fixture,job.token,if passed {"passed"}else{"failed"},now])?==1,"Stale verification claim");
        Ok(())
    }
    pub fn retry_runtime_check(&mut self, digest: &str, now: i64) -> Result<()> {
        ensure!(self.db.execute("UPDATE runtime_checks SET state='queued',available_at=?3 WHERE digest=?1 AND fixture=?2 AND state!='running'",params![digest,fixture_id(),now])?==1,"Unknown or running verification");
        Ok(())
    }
    pub fn runtime_check_status(&self) -> Result<serde_json::Value> {
        let mut q=self.db.prepare("SELECT binary,digest,fixture,version,state,last_result,checked_at FROM runtime_checks ORDER BY available_at DESC,digest LIMIT 100")?;
        Ok(serde_json::json!(q.query_map([],|r|Ok(serde_json::json!({"binary":r.get::<_,String>(0)?,"binary_sha256":r.get::<_,String>(1)?,"fixture":r.get::<_,String>(2)?,"version":r.get::<_,String>(3)?,"state":r.get::<_,String>(4)?,"last_result":r.get::<_,Option<String>>(5)?,"checked_at":r.get::<_,Option<i64>>(6)?})))?.collect::<rusqlite::Result<Vec<_>>>()?))
    }
}
pub async fn process_one(root: &Path) -> Result<bool> {
    let mut state = State::open(&root.join("state.sqlite"))?;
    let Some(job) = state.claim_runtime_check(crate::ingestion::now())? else {
        return Ok(false);
    };
    let passed = if job.fixture == fixture_id()
        && fingerprint(Path::new(&job.binary)).is_ok_and(|d| d == job.digest)
    {
        match crate::runtime_probe::run(Path::new(&job.binary), root).await {
            Ok(()) => fingerprint(Path::new(&job.binary)).is_ok_and(|d| d == job.digest),
            Err(error) => {
                eprintln!("Synthetic runtime verification failed: {error}");
                false
            }
        }
    } else {
        false
    };
    state.finish_runtime_check(&job, passed, crate::ingestion::now())?;
    Ok(true)
}
pub struct Runner(tokio::task::JoinHandle<()>);
impl Drop for Runner {
    fn drop(&mut self) {
        self.0.abort();
    }
}
pub fn start(root: PathBuf) -> Runner {
    Runner(tokio::spawn(async move {
        loop {
            if process_one(&root).await.is_err() {
                eprintln!("Runtime verification job failed; inspect runtime-check-status");
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }))
}
