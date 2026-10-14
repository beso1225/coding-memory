use anyhow::Result;
use coding_memory::{
    consolidation_state::Batch,
    domain::{Evidence, RunKey},
    evidence, extraction,
    state::State,
    worker::{Config, Models, Ownership, Source},
};
use serde_json::json;
use std::{fs, path::Path};
use tempfile::tempdir;
fn key(thread: &str) -> RunKey {
    RunKey {
        store: "fixture".into(),
        thread: thread.into(),
        turn: "r".into(),
    }
}
fn ready(state: &mut State, root: &Path) -> Result<()> {
    let b = Evidence {
        schema_version: 1,
        run: key("ready"),
        project: "alpha".into(),
        runtime_version: "fixture".into(),
        outcome: "completed".into(),
        started_at: 1,
        completed_at: 2,
        cwd: None,
        terminal_error: None,
        items: vec![json!({"id":"U","type":"userMessage","text":"Use leased jobs"})],
        coverage: vec![],
    };
    let digest = evidence::save(root, &b)?;
    state.enqueue(&b.run, "alpha", "/repo", 1)?;
    let c = state.claim(2, 100)?.unwrap();
    state.finish(&c, "completed", Some(&digest), 3)?;
    state.enqueue_extractions("alpha", 4)?;
    Ok(())
}
fn stage(state: &mut State, root: &Path) -> Result<()> {
    ready(state, root)?;
    let c = state.claim_extraction("alpha", 5, 100)?.unwrap();
    let b = evidence::load(root, &c.evidence)?;
    let batch = extraction::decode(
        &serde_json::to_vec(
            &json!({"schema_version":1,"role":"extraction","project_id":"alpha","candidates":[{"kind":"constraint","title":"Use leased jobs","claim":"Use leased jobs.","scope":{"kind":"project","project_id":"alpha"},"conditions":[],"verification":"user_stated","evidence":[{"bundle":c.evidence,"item":"U"}]}]}),
        )?,
        &b,
        &c.evidence,
    )?;
    state.finish_extraction(&c, &batch, &json!({}), 6)?;
    Ok(())
}
#[test]
fn singleton_lock_survives_owner_file_and_releases_with_its_session() -> Result<()> {
    let d = tempdir()?;
    let owner = Ownership::acquire(d.path())?;
    assert!(Ownership::acquire(d.path()).is_err());
    let mut state = State::open(&d.path().join("state.sqlite"))?;
    state.begin_worker(&owner, 1)?;
    drop(owner);
    assert!(Ownership::acquire(d.path()).is_err());
    drop(state);
    assert!(Ownership::acquire(d.path()).is_ok());
    Ok(())
}
#[cfg(unix)]
#[test]
fn owner_release_unlocks_even_when_a_child_inherits_the_descriptor() -> Result<()> {
    let d = tempdir()?;
    let owner = Ownership::acquire(d.path())?;
    let mut pipe = [0; 2];
    assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        // Only async-signal-safe syscalls after fork in this multithreaded test process.
        unsafe {
            libc::close(pipe[1]);
            let mut byte = 0u8;
            libc::read(pipe[0], (&mut byte as *mut u8).cast(), 1);
            libc::_exit(0);
        }
    }
    unsafe {
        libc::close(pipe[0]);
    }
    drop(owner);
    let result = Ownership::acquire(d.path());
    unsafe {
        libc::close(pipe[1]);
        libc::waitpid(pid, std::ptr::null_mut(), 0);
    }
    assert!(
        result.is_ok(),
        "Inherited descriptor retained the released lock"
    );
    Ok(())
}
#[test]
fn restart_recovers_only_worker_capture_claims_and_fences_late_results() -> Result<()> {
    let d = tempdir()?;
    let mut state = State::open(&d.path().join("state.sqlite"))?;
    state.register_project("alpha", "/repo", 1)?;
    state.enqueue(&key("00-worker"), "alpha", "/repo", 1)?;
    state.enqueue(&key("01-manual"), "alpha", "/repo", 1)?;
    let owner = Ownership::acquire(d.path())?;
    state.begin_worker(&owner, 2)?;
    let worker = state.claim(2, 100)?.unwrap();
    let mut manual = State::open(&d.path().join("state.sqlite"))?;
    let other = manual.claim(2, 100)?.unwrap();
    drop(state);
    drop(owner);
    let owner = Ownership::acquire(d.path())?;
    let mut state = State::open(&d.path().join("state.sqlite"))?;
    state.begin_worker(&owner, 3)?;
    assert!(state.finish(&worker, "failed", None, 4).is_err());
    assert_eq!(
        state
            .status()?
            .iter()
            .find(|r| r.key == worker.key)
            .unwrap()
            .state,
        "queued"
    );
    assert_eq!(
        state
            .status()?
            .iter()
            .find(|r| r.key == other.key)
            .unwrap()
            .state,
        "capturing"
    );
    manual.finish(&other, "failed", None, 4)?;
    Ok(())
}
#[test]
fn extraction_restarts_preserve_the_three_attempt_limit() -> Result<()> {
    let d = tempdir()?;
    let mut state = State::open(&d.path().join("state.sqlite"))?;
    state.register_project("alpha", "/repo", 1)?;
    ready(&mut state, d.path())?;
    drop(state);
    for attempt in 1..=3 {
        let owner = Ownership::acquire(d.path())?;
        let mut state = State::open(&d.path().join("state.sqlite"))?;
        state.begin_worker(&owner, 10 + attempt)?;
        assert_eq!(
            state
                .claim_extraction("alpha", 10 + attempt, 100)?
                .unwrap()
                .attempts,
            attempt
        );
        drop(state);
        drop(owner);
    }
    let owner = Ownership::acquire(d.path())?;
    let mut state = State::open(&d.path().join("state.sqlite"))?;
    state.begin_worker(&owner, 20)?;
    assert!(state.claim_extraction("alpha", 20, 100)?.is_none());
    assert_eq!(state.extraction_status("alpha")?[0].state, "exhausted");
    Ok(())
}
#[test]
fn consolidation_transient_retries_are_bounded_and_remain_deliberately_retryable() -> Result<()> {
    let d = tempdir()?;
    let mut state = State::open(&d.path().join("state.sqlite"))?;
    state.register_project("alpha", "/repo", 1)?;
    stage(&mut state, d.path())?;
    let mut last: Option<Batch> = None;
    for attempt in 0..3 {
        let now = 10 + attempt * 20;
        let batch = state.reserve_batch("alpha", now, true)?.unwrap();
        state.hold_batch_at(batch.id, "generation_process_failure", &json!({}), now)?;
        state.retry_transient_batches("other-project", now + 10)?;
        assert_eq!(state.consolidation_status()?.last().unwrap().state, "held");
        state.retry_transient_batches("alpha", now + 9)?;
        assert_eq!(state.consolidation_status()?.last().unwrap().state, "held");
        state.retry_transient_batches("alpha", now + 10)?;
        last = Some(batch);
    }
    assert_eq!(
        state.consolidation_status()?.last().unwrap().state,
        "exhausted"
    );
    assert!(state.reserve_batch("alpha", 100, true)?.is_none());
    state.retry_batch(last.unwrap().id, 100)?;
    assert!(state.reserve_batch("alpha", 100, true)?.is_some());
    Ok(())
}
fn configured(root: &Path) -> Result<(State, Config)> {
    let project = root.join("repo");
    let home = root.join("collection-home");
    let vault = root.join("vault");
    fs::create_dir(&project)?;
    fs::create_dir(&home)?;
    fs::create_dir(&vault)?;
    assert!(
        std::process::Command::new("git")
            .arg("init")
            .arg(&project)
            .output()?
            .status
            .success()
    );
    let mut state = State::open(&root.join("state.sqlite"))?;
    let p = coding_memory::project::resolve(&project)?;
    state.register_project(&p.id, p.repository.to_str().unwrap(), 1)?;
    let config = Config {
        schema_version: 1,
        vault,
        sources: vec![Source {
            project,
            codex_home: home,
            codex_binary: "/usr/bin/true".into(),
        }],
        generation_binary: "/usr/bin/true".into(),
        models: Models {
            extraction: "synthetic".into(),
            consolidation: "synthetic".into(),
            review: "synthetic".into(),
        },
        poll_seconds: 30,
        timeout_seconds: 30,
        min_ready: 5,
        max_age_seconds: 600,
        executable_path: vec!["/usr/bin".into(), "/bin".into()],
        loopback: None,
    };
    Ok((state, config))
}
#[test]
fn config_rejects_invalid_or_unregistered_sources_before_processing() -> Result<()> {
    let d = tempdir()?;
    let (state, config) = configured(d.path())?;
    config.validate(d.path(), &state)?;
    let mut bad = config.clone();
    bad.timeout_seconds = 0;
    assert!(bad.validate(d.path(), &state).is_err());
    let mut bad = config.clone();
    bad.sources.push(bad.sources[0].clone());
    assert!(bad.validate(d.path(), &state).is_err());
    let mut bad = config.clone();
    bad.generation_binary = "relative-codex".into();
    assert!(bad.validate(d.path(), &state).is_err());
    let mut bad = config;
    bad.models.extraction = "\0".into();
    assert!(bad.validate(d.path(), &state).is_err());
    Ok(())
}
#[tokio::test]
async fn reader_failure_is_persisted_without_disabling_status_or_retrieval() -> Result<()> {
    let d = tempdir()?;
    let (mut state, config) = configured(d.path())?;
    let report = coding_memory::worker::cycle(
        &mut state,
        d.path(),
        &config,
        Path::new(env!("CARGO_BIN_EXE_coding-memory")),
    )
    .await?;
    assert_eq!(report.failures[0].stage, "reader_start_failed");
    assert!(state.worker_status()?["last_cycle"]["completed_at"].is_number());
    Ok(())
}
#[cfg(target_os = "macos")]
#[test]
fn launchd_argv_and_xml_work_with_spaces_and_metacharacters() -> Result<()> {
    let d = tempdir()?;
    let root = d.path().join("state & space");
    fs::create_dir(&root)?;
    let (_state, config) = configured(&root)?;
    let config_path = root.join("worker config.json");
    fs::write(&config_path, serde_json::to_vec(&config)?)?;
    let invalid_binary = root.join("non-executable");
    fs::write(&invalid_binary, "not executable")?;
    assert!(coding_memory::service::plist(&root, &config_path, &invalid_binary, "test").is_err());
    let plist = coding_memory::service::plist(
        &root,
        &config_path,
        Path::new(env!("CARGO_BIN_EXE_coding-memory")),
        "com.beso1225.coding-memory.test",
    )?;
    assert!(plist.contains("&amp;"));
    assert!(!plist.contains("/bin/sh"));
    let output = root.join("agent.plist");
    coding_memory::service::write(&output, &plist)?;
    assert!(
        std::process::Command::new("/usr/bin/plutil")
            .args(["-lint"])
            .arg(output)
            .output()?
            .status
            .success()
    );
    Ok(())
}
#[test]
fn restart_does_not_reclaim_a_manual_successor_to_an_expired_worker_lease() -> Result<()> {
    let d = tempdir()?;
    let mut s = State::open(&d.path().join("state.sqlite"))?;
    s.register_project("alpha", "/repo", 1)?;
    s.enqueue(&key("same"), "alpha", "/repo", 1)?;
    let owner = Ownership::acquire(d.path())?;
    s.begin_worker(&owner, 2)?;
    let old = s.claim(2, 10)?.unwrap();
    drop(s);
    drop(owner);
    let mut manual = State::open(&d.path().join("state.sqlite"))?;
    let active = manual.claim(12, 100)?.unwrap();
    assert!(active.token > old.token);
    let owner = Ownership::acquire(d.path())?;
    let mut s = State::open(&d.path().join("state.sqlite"))?;
    s.begin_worker(&owner, 13)?;
    assert_eq!(s.status()?[0].state, "capturing");
    manual.finish(&active, "failed", None, 14)?;
    Ok(())
}
#[test]
fn interrupted_consolidation_releases_only_worker_reserved_candidates() -> Result<()> {
    let d = tempdir()?;
    let mut s = State::open(&d.path().join("state.sqlite"))?;
    s.register_project("alpha", "/repo", 1)?;
    stage(&mut s, d.path())?;
    let owner = Ownership::acquire(d.path())?;
    s.begin_worker(&owner, 10)?;
    let job = s.reserve_batch("alpha", 10, true)?.unwrap();
    drop(s);
    drop(owner);
    let owner = Ownership::acquire(d.path())?;
    let mut s = State::open(&d.path().join("state.sqlite"))?;
    s.begin_worker(&owner, 11)?;
    assert_eq!(s.consolidation_status()?[0].id, job.id);
    assert_eq!(
        s.consolidation_status()?[0].reason.as_deref(),
        Some("worker_interrupted")
    );
    assert_eq!(s.candidates("alpha")?[0].state, "held");
    s.retry_transient_batches("alpha", 21)?;
    assert_eq!(s.candidates("alpha")?[0].state, "ready");
    Ok(())
}
