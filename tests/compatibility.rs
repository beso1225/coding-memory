use coding_memory::codex;
#[test]
fn provisional_versions_use_semantic_minimums_not_an_exact_allowlist() {
    for v in ["0.162.0-alpha.17.2", "0.162.1", "0.163.0", "1.0.0"] {
        assert!(codex::supported(v), "{v}");
    }
    for v in ["0.162.0-alpha.2", "0.161.9", "invalid", "0.162.1\n"] {
        assert!(!codex::supported(v), "{v}");
    }
    assert!(codex::history_version(&serde_json::json!({"cliVersion":"0.162.0-alpha.2"})).is_ok());
    assert!(codex::history_version(&serde_json::json!({"cliVersion":"0.162.0-alpha.1"})).is_err());
}

#[test]
fn verification_jobs_are_deduplicated_fenced_and_expire_weekly() -> anyhow::Result<()> {
    use coding_memory::{
        compatibility::{INTERVAL, Identity},
        state::State,
    };
    let d = tempfile::tempdir()?;
    let mut state = State::open(&d.path().join("state.sqlite"))?;
    let identity = Identity {
        binary: "/usr/bin/true".into(),
        digest: "a".repeat(64),
        version: "0.163.0".into(),
    };
    assert_eq!(
        state.observe_runtime(&identity, 10)?.validation,
        "provisional"
    );
    state.observe_runtime(&identity, 10)?;
    let job = state.claim_runtime_check(10)?.unwrap();
    assert!(state.claim_runtime_check(10)?.is_none());
    let newer = state.claim_runtime_check(190)?.unwrap();
    assert!(newer.token > job.token);
    assert!(state.finish_runtime_check(&job, true, 191).is_err());
    state.finish_runtime_check(&newer, true, 191)?;
    assert_eq!(
        state.observe_runtime(&identity, 192)?.validation,
        "verified"
    );
    assert!(state.claim_runtime_check(192)?.is_none());
    assert_eq!(
        state.observe_runtime(&identity, 191 + INTERVAL)?.validation,
        "provisional"
    );
    let job = state.claim_runtime_check(191 + INTERVAL)?.unwrap();
    state.finish_runtime_check(&job, false, 192 + INTERVAL)?;
    assert_eq!(
        state.observe_runtime(&identity, 193 + INTERVAL)?.validation,
        "failed"
    );
    assert!(state.claim_runtime_check(193 + INTERVAL)?.is_none());
    state.retry_runtime_check(&identity.digest, 194 + INTERVAL)?;
    assert_eq!(
        state.observe_runtime(&identity, 194 + INTERVAL)?.validation,
        "failed"
    );
    let job = state.claim_runtime_check(194 + INTERVAL)?.unwrap();
    state.finish_runtime_check(&job, true, 195 + INTERVAL)?;
    assert_eq!(
        state.observe_runtime(&identity, 196 + INTERVAL)?.validation,
        "verified"
    );
    let mut changed = identity.clone();
    changed.digest = "b".repeat(64);
    assert_eq!(
        state.observe_runtime(&changed, 197 + INTERVAL)?.validation,
        "provisional"
    );
    assert!(state.claim_runtime_check(197 + INTERVAL)?.is_some());
    Ok(())
}

#[test]
fn a_fixture_change_rechecks_but_does_not_erase_a_known_failure() -> anyhow::Result<()> {
    use coding_memory::{compatibility::Identity, state::State};
    let d = tempfile::tempdir()?;
    let path = d.path().join("state.sqlite");
    let mut state = State::open(&path)?;
    let identity = Identity {
        binary: "/usr/bin/true".into(),
        digest: "c".repeat(64),
        version: "0.163.0".into(),
    };
    let db = rusqlite::Connection::open(&path)?;
    db.execute("INSERT INTO runtime_checks(digest,fixture,binary,version,state,last_result,checked_at,available_at) VALUES(?1,'previous','/usr/bin/true','0.163.0','complete','failed',1000,10)",[&identity.digest])?;
    db.execute("INSERT INTO runtime_checks(digest,fixture,binary,version,state,available_at) VALUES(?1,'obsolete-pending','/usr/bin/true','0.163.0','queued',9)",[&identity.digest])?;
    assert_eq!(state.observe_runtime(&identity, 11)?.validation, "failed");
    let job = state.claim_runtime_check(11)?.unwrap();
    assert_eq!(job.fixture, coding_memory::compatibility::fixture_id());
    state.finish_runtime_check(&job, true, 12)?;
    assert_eq!(state.observe_runtime(&identity, 13)?.validation, "verified");
    assert!(state.claim_runtime_check(13)?.is_none());
    Ok(())
}

#[test]
fn moving_identical_binary_refreshes_queued_and_failed_check_paths() -> anyhow::Result<()> {
    use coding_memory::{compatibility::Identity, state::State};
    let d = tempfile::tempdir()?;
    let path = d.path().join("state.sqlite");
    let mut state = State::open(&path)?;
    let old = d.path().join("old");
    let new = d.path().join("new");
    std::fs::write(&old, b"identical synthetic executable")?;
    let mut identity = Identity {
        digest: coding_memory::compatibility::fingerprint(&old)?,
        binary: old.clone(),
        version: "0.162.1".into(),
    };
    state.observe_runtime(&identity, 10)?;
    std::fs::rename(&old, &new)?;
    identity.binary = new.clone();
    state.observe_runtime(&identity, 11)?;
    let job = state.claim_runtime_check(11)?.unwrap();
    assert_eq!(job.binary, new.to_str().unwrap());
    assert_eq!(
        coding_memory::compatibility::fingerprint(std::path::Path::new(&job.binary))?,
        job.digest
    );
    state.finish_runtime_check(&job, false, 12)?;
    let final_path = d.path().join("final");
    std::fs::rename(&new, &final_path)?;
    identity.binary = final_path.clone();
    identity.version = "0.162.1+relocated".into();
    assert_eq!(state.observe_runtime(&identity, 13)?.validation, "failed");
    state.retry_runtime_check(&identity.digest, 13)?;
    let job = state.claim_runtime_check(13)?.unwrap();
    assert_eq!(job.binary, final_path.to_str().unwrap());
    assert_eq!(job.version, identity.version);
    state.finish_runtime_check(&job, true, 14)?;
    assert_eq!(state.observe_runtime(&identity, 15)?.validation, "verified");
    Ok(())
}
