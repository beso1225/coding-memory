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
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
pub const INTERVAL: i64 = 7 * 24 * 60 * 60;
const LEASE: i64 = 180;
const CHECK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(150);
const HASH_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
pub fn fixture_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        hash(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/capture-fixture-inputs"
        )))
    })
}
#[derive(Clone, Debug)]
pub struct Identity {
    pub binary: PathBuf,
    pub digest: String,
    pub version: String,
    pub stamp: Option<ExecutableStamp>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutableStamp {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl ExecutableStamp {
    fn metadata(metadata: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
    pub fn read(path: &Path) -> Result<Self> {
        let metadata = std::fs::symlink_metadata(path)?;
        ensure!(metadata.is_file(), "Not a binary file");
        Ok(Self::metadata(&metadata))
    }
}
struct Fingerprint {
    digest: String,
    stamp: ExecutableStamp,
}
pub fn fingerprint(binary: &Path) -> Result<String> {
    Ok(fingerprint_file(
        binary,
        Instant::now() + HASH_DEADLINE,
        &AtomicBool::new(false),
    )?
    .digest)
}
fn fingerprint_file(
    binary: &Path,
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<Fingerprint> {
    ensure!(
        !cancelled.load(Ordering::Relaxed) && Instant::now() < deadline,
        "Fingerprint cancelled"
    );
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    let mut file = options.open(binary)?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "Not a binary file");
    let stamp = ExecutableStamp::metadata(&metadata);
    let digest = hash_stream(&mut file, deadline, cancelled)?;
    ensure!(
        stamp == ExecutableStamp::metadata(&file.metadata()?)
            && stamp == ExecutableStamp::read(binary)?,
        "Executable changed during fingerprint"
    );
    Ok(Fingerprint { digest, stamp })
}
fn hash_stream(
    reader: &mut impl Read,
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    let mut total = 0;
    loop {
        ensure!(
            !cancelled.load(Ordering::Relaxed) && Instant::now() < deadline,
            "Fingerprint cancelled"
        );
        let n = reader.read(&mut buf)?;
        // A filesystem syscall cannot be interrupted here. Never start another read after cancellation.
        ensure!(
            !cancelled.load(Ordering::Relaxed) && Instant::now() < deadline,
            "Fingerprint cancelled"
        );
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
    let before = executable_stamp(&binary).await?;
    let version = crate::generation::version(&binary, home)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.reason()))?;
    ensure!(
        crate::codex::supported(&version),
        "Reader below minimum version"
    );
    let fingerprint = fingerprint_bounded(&binary).await?;
    ensure!(
        before == fingerprint.stamp,
        "Executable changed during identification"
    );
    Ok(Identity {
        digest: fingerprint.digest,
        binary,
        version,
        stamp: Some(fingerprint.stamp),
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
pub(crate) fn observe_runtime_on(
    db: &rusqlite::Connection,
    identity: &Identity,
    now: i64,
) -> Result<RuntimeAudit> {
    let binary = identity
        .binary
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Non-UTF8 binary"))?;
    // The row remembers the latest locator for future claims. An existing job
    // retains its own locator and token; another identical copy must not fence it.
    db.execute("INSERT INTO runtime_checks(digest,fixture,binary,version,state,available_at) VALUES(?1,?2,?3,?4,'queued',?5) ON CONFLICT(digest,fixture) DO UPDATE SET binary=excluded.binary,version=excluded.version",params![identity.digest,fixture_id(),binary,identity.version,now])?;
    db.execute("UPDATE runtime_checks SET state='queued',available_at=?3 WHERE digest=?1 AND fixture=?2 AND state='complete' AND (checked_at>?3 OR checked_at<=?4)",params![identity.digest,fixture_id(),now,now.saturating_sub(INTERVAL)])?;
    let (result, checked): (Option<String>, Option<i64>) = db.query_row(
        "SELECT last_result,checked_at FROM runtime_checks WHERE digest=?1 AND fixture=?2",
        params![identity.digest, fixture_id()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let latest: Option<String> = db.query_row("SELECT last_result FROM runtime_checks WHERE digest=?1 AND last_result IS NOT NULL ORDER BY checked_at DESC,(fixture=?2) DESC LIMIT 1",params![identity.digest,fixture_id()],|r|r.get(0)).optional()?;
    let status = if result.as_deref() == Some("failed")
        || (result.is_none() && latest.as_deref() == Some("failed"))
    {
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
impl State {
    pub fn observe_runtime(&mut self, identity: &Identity, now: i64) -> Result<RuntimeAudit> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let audit = observe_runtime_on(&tx, identity, now)?;
        tx.commit()?;
        Ok(audit)
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
    pub fn requeue_runtime_locator(&mut self, job: &Job, now: i64) -> Result<bool> {
        // An unavailable old locator is retried at the latest observed one. Valid
        // fixture failures still finish normally, even when copies alternate.
        Ok(self.db.execute("UPDATE runtime_checks SET state='queued',token=token+1,lease_until=NULL,available_at=?6 WHERE digest=?1 AND fixture=?2 AND token=?3 AND state='running' AND lease_until>?6 AND (binary!=?4 OR version!=?5)", params![job.digest,job.fixture,job.token,job.binary,job.version,now])? == 1)
    }
    pub fn retry_runtime_check(&mut self, digest: &str, now: i64) -> Result<()> {
        ensure!(self.db.execute("UPDATE runtime_checks SET state='queued',token=token+1,lease_until=NULL,available_at=?3 WHERE digest=?1 AND fixture=?2 AND (state!='running' OR lease_until<=?3)",params![digest,fixture_id(),now])?==1,"Unknown or running verification");
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
    finish_check(&mut state, &job, check(&job, root), CHECK_DEADLINE).await?;
    Ok(true)
}
struct CancelHash(Arc<AtomicBool>);
impl Drop for CancelHash {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
async fn bounded_hash<T: Send + 'static>(
    slots: Arc<tokio::sync::Semaphore>,
    duration: Duration,
    operation: impl FnOnce(Instant, Arc<AtomicBool>) -> Result<T> + Send + 'static,
) -> Result<T> {
    let deadline = Instant::now() + duration;
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel = CancelHash(cancelled.clone());
    tokio::time::timeout(duration, async move {
        let permit = slots.acquire_owned().await?;
        tokio::task::spawn_blocking(move || {
            // Hold the slot until even a stalled OS read actually returns.
            let _permit = permit;
            operation(deadline, cancelled)
        })
        .await?
    })
    .await?
}
fn hash_slots() -> Arc<tokio::sync::Semaphore> {
    static SLOTS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    SLOTS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2)))
        .clone()
}
pub(crate) async fn executable_stamp(binary: &Path) -> Result<ExecutableStamp> {
    let path = binary.to_owned();
    bounded_hash(hash_slots(), HASH_DEADLINE, move |deadline, cancelled| {
        ensure!(
            !cancelled.load(Ordering::Relaxed) && Instant::now() < deadline,
            "Executable check cancelled"
        );
        let stamp = ExecutableStamp::read(&path)?;
        ensure!(
            !cancelled.load(Ordering::Relaxed) && Instant::now() < deadline,
            "Executable check cancelled"
        );
        Ok(stamp)
    })
    .await
}
async fn fingerprint_bounded(binary: &Path) -> Result<Fingerprint> {
    let path = binary.to_owned();
    bounded_hash(hash_slots(), HASH_DEADLINE, move |deadline, cancelled| {
        fingerprint_file(&path, deadline, &cancelled)
    })
    .await
}
enum Verification {
    Passed,
    Failed,
    Unavailable,
}
async fn check(job: &Job, root: &Path) -> Result<Verification> {
    if job.fixture != fixture_id() {
        return Ok(Verification::Failed);
    }
    let Ok(fingerprint) = fingerprint_bounded(Path::new(&job.binary)).await else {
        return Ok(Verification::Unavailable);
    };
    if fingerprint.digest != job.digest {
        return Ok(Verification::Unavailable);
    }
    let result = crate::runtime_probe::run(
        &Identity {
            binary: job.binary.clone().into(),
            digest: job.digest.clone(),
            version: job.version.clone(),
            stamp: Some(fingerprint.stamp),
        },
        root,
    )
    .await;
    // Distinguish a moved/replaced locator from an actual fixture failure.
    if !fingerprint_bounded(Path::new(&job.binary))
        .await
        .is_ok_and(|d| d.digest == job.digest)
    {
        return Ok(Verification::Unavailable);
    }
    if let Err(error) = result {
        eprintln!("Synthetic runtime verification failed: {error}");
        return Ok(Verification::Failed);
    }
    Ok(Verification::Passed)
}
async fn finish_check(
    state: &mut State,
    job: &Job,
    check: impl std::future::Future<Output = Result<Verification>>,
    deadline: Duration,
) -> Result<()> {
    let now_result = tokio::time::timeout(deadline, check).await;
    let now = crate::ingestion::now();
    match now_result {
        Ok(Ok(Verification::Passed)) => state.finish_runtime_check(job, true, now),
        Ok(Ok(Verification::Failed)) => state.finish_runtime_check(job, false, now),
        _ if state.requeue_runtime_locator(job, now)? => Ok(()),
        _ => state.finish_runtime_check(job, false, now),
    }
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

#[cfg(test)]
mod deadline_tests {
    use super::*;
    #[tokio::test]
    async fn stalled_check_records_failure_before_its_lease_expires() -> Result<()> {
        let d = tempfile::tempdir()?;
        let mut state = State::open(&d.path().join("state.sqlite"))?;
        let identity = Identity {
            binary: "/usr/bin/true".into(),
            digest: "e".repeat(64),
            version: "0.162.1".into(),
            stamp: None,
        };
        let now = crate::ingestion::now();
        state.observe_runtime(&identity, now)?;
        let job = state.claim_runtime_check(now)?.unwrap();
        let lease: i64 = state.db.query_row(
            "SELECT lease_until FROM runtime_checks WHERE digest=?1",
            [&identity.digest],
            |r| r.get(0),
        )?;
        assert!(now + (CHECK_DEADLINE.as_secs() as i64) < lease);
        finish_check(
            &mut state,
            &job,
            std::future::pending(),
            std::time::Duration::from_millis(5),
        )
        .await?;
        assert_eq!(
            state
                .observe_runtime(&identity, crate::ingestion::now())?
                .validation,
            "failed"
        );
        assert!(
            state
                .claim_runtime_check(crate::ingestion::now())?
                .is_none()
        );
        Ok(())
    }
    #[tokio::test]
    async fn identification_rejects_atomic_replacement_during_version_probe() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir()?;
        let binary = d.path().join("changing.py");
        std::fs::write(
            &binary,
            r#"#!/usr/bin/env python3
from pathlib import Path
import os
p=Path(__file__)
n=p.with_suffix('.next')
n.write_bytes(p.read_bytes()+b'\n')
n.chmod(0o700)
os.replace(n,p)
print('codex-cli 0.162.1')
"#,
        )?;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))?;
        assert!(
            identify(&binary, d.path())
                .await
                .unwrap_err()
                .to_string()
                .contains("Executable changed")
        );
        Ok(())
    }
    #[test]
    fn hashing_stops_after_cancelled_or_overdue_read() {
        struct SlowRead<'a> {
            flag: &'a AtomicBool,
            reads: usize,
            cancel: bool,
        }
        impl Read for SlowRead<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                self.reads += 1;
                if self.cancel {
                    self.flag.store(true, Ordering::Relaxed);
                } else {
                    std::thread::sleep(Duration::from_millis(3));
                }
                buf[0] = 0;
                Ok(1)
            }
        }
        for cancel in [true, false] {
            let flag = AtomicBool::new(false);
            let mut reader = SlowRead {
                flag: &flag,
                reads: 0,
                cancel,
            };
            let deadline = Instant::now()
                + if cancel {
                    Duration::from_secs(1)
                } else {
                    Duration::from_millis(1)
                };
            assert!(hash_stream(&mut reader, deadline, &flag).is_err());
            assert_eq!(reader.reads, 1);
        }
    }
    #[tokio::test]
    async fn stalled_hashes_keep_their_slot_until_the_os_read_returns() -> Result<()> {
        let slots = Arc::new(tokio::sync::Semaphore::new(1));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (release, stalled) = std::sync::mpsc::channel::<()>();
        let counter = calls.clone();
        assert!(
            bounded_hash(slots.clone(), Duration::from_millis(50), move |_, _| {
                counter.fetch_add(1, Ordering::Relaxed);
                stalled.recv()?;
                Ok(())
            })
            .await
            .is_err()
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        for _ in 0..4 {
            let counter = calls.clone();
            assert!(
                bounded_hash(slots.clone(), Duration::from_millis(2), move |_, _| {
                    counter.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                })
                .await
                .is_err()
            );
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        release.send(())?;
        let _permit = tokio::time::timeout(Duration::from_secs(1), slots.acquire_owned()).await??;
        Ok(())
    }
}
