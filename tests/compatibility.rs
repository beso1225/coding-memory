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
    let newer = state.claim_runtime_check(130)?.unwrap();
    assert!(newer.token > job.token);
    assert!(state.finish_runtime_check(&job, true, 131).is_err());
    state.finish_runtime_check(&newer, true, 131)?;
    assert_eq!(
        state.observe_runtime(&identity, 132)?.validation,
        "verified"
    );
    assert!(state.claim_runtime_check(132)?.is_none());
    assert_eq!(
        state.observe_runtime(&identity, 131 + INTERVAL)?.validation,
        "provisional"
    );
    let job = state.claim_runtime_check(131 + INTERVAL)?.unwrap();
    state.finish_runtime_check(&job, false, 132 + INTERVAL)?;
    assert_eq!(
        state.observe_runtime(&identity, 133 + INTERVAL)?.validation,
        "failed"
    );
    assert!(state.claim_runtime_check(133 + INTERVAL)?.is_none());
    state.retry_runtime_check(&identity.digest, 134 + INTERVAL)?;
    assert_eq!(
        state.observe_runtime(&identity, 134 + INTERVAL)?.validation,
        "failed"
    );
    let job = state.claim_runtime_check(134 + INTERVAL)?.unwrap();
    state.finish_runtime_check(&job, true, 135 + INTERVAL)?;
    assert_eq!(
        state.observe_runtime(&identity, 136 + INTERVAL)?.validation,
        "verified"
    );
    let mut changed = identity.clone();
    changed.digest = "b".repeat(64);
    assert_eq!(
        state.observe_runtime(&changed, 137 + INTERVAL)?.validation,
        "provisional"
    );
    assert!(state.claim_runtime_check(137 + INTERVAL)?.is_some());
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
    db.execute("INSERT INTO runtime_checks(digest,fixture,binary,version,state,last_result,checked_at,available_at) VALUES(?1,'previous','/usr/bin/true','0.163.0','complete','failed',10,10)",[&identity.digest])?;
    db.execute("INSERT INTO runtime_checks(digest,fixture,binary,version,state,available_at) VALUES(?1,'obsolete-pending','/usr/bin/true','0.163.0','queued',9)",[&identity.digest])?;
    assert_eq!(state.observe_runtime(&identity, 11)?.validation, "failed");
    let job = state.claim_runtime_check(11)?.unwrap();
    assert_eq!(job.fixture, coding_memory::compatibility::fixture_id());
    state.finish_runtime_check(&job, true, 12)?;
    assert_eq!(state.observe_runtime(&identity, 13)?.validation, "verified");
    assert!(state.claim_runtime_check(13)?.is_none());
    Ok(())
}
