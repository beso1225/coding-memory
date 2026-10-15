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
        stamp: None,
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
        stamp: None,
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
        stamp: None,
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

#[test]
fn relocation_fences_inflight_results_without_erasing_known_failure() -> anyhow::Result<()> {
    use coding_memory::{compatibility::Identity, state::State};
    let d = tempfile::tempdir()?;
    let path = d.path().join("state.sqlite");
    let mut observer = State::open(&path)?;
    let mut verifier = State::open(&path)?;
    let mut identity = Identity {
        binary: d.path().join("old"),
        digest: "d".repeat(64),
        version: "0.162.1".into(),
        stamp: None,
    };
    std::fs::write(&identity.binary, b"synthetic executable")?;
    observer.observe_runtime(&identity, 10)?;
    let prior = verifier.claim_runtime_check(10)?.unwrap();
    verifier.finish_runtime_check(&prior, false, 11)?;
    observer.retry_runtime_check(&identity.digest, 12)?;
    let inflight = verifier.claim_runtime_check(12)?.unwrap();
    // Observing the same path must preserve an active verifier's claim.
    observer.observe_runtime(&identity, 13)?;
    assert!(observer.claim_runtime_check(13)?.is_none());
    let new = d.path().join("new");
    std::fs::rename(&identity.binary, &new)?;
    identity.binary = new;
    assert_eq!(
        observer.observe_runtime(&identity, 14)?.validation,
        "failed"
    );
    assert!(observer.claim_runtime_check(14)?.is_none());
    // Once the verifier finds that its old locator is gone, retry at the new one.
    assert!(verifier.requeue_runtime_locator(&inflight, 15)?);
    let replacement = observer.claim_runtime_check(15)?.unwrap();
    assert_eq!(replacement.binary, identity.binary.to_str().unwrap());
    assert!(replacement.token > inflight.token);
    assert!(verifier.finish_runtime_check(&inflight, false, 15).is_err());
    assert!(verifier.finish_runtime_check(&inflight, true, 15).is_err());
    observer.finish_runtime_check(&replacement, true, 16)?;
    assert_eq!(
        observer.observe_runtime(&identity, 17)?.validation,
        "verified"
    );
    Ok(())
}

#[test]
fn explicit_retry_reclaims_only_expired_verification_leases() -> anyhow::Result<()> {
    use coding_memory::{compatibility::Identity, state::State};
    let d = tempfile::tempdir()?;
    let mut state = State::open(&d.path().join("state.sqlite"))?;
    let identity = Identity {
        binary: "/usr/bin/true".into(),
        digest: "f".repeat(64),
        version: "0.162.1".into(),
        stamp: None,
    };
    state.observe_runtime(&identity, 10)?;
    let old = state.claim_runtime_check(10)?.unwrap();
    assert!(state.retry_runtime_check(&identity.digest, 189).is_err());
    assert!(state.claim_runtime_check(189)?.is_none());
    state.retry_runtime_check(&identity.digest, 190)?;
    let replacement = state.claim_runtime_check(190)?.unwrap();
    assert!(replacement.token > old.token);
    assert!(state.finish_runtime_check(&old, true, 191).is_err());
    state.finish_runtime_check(&replacement, true, 192)?;
    assert_eq!(
        state.observe_runtime(&identity, 193)?.validation,
        "verified"
    );
    Ok(())
}

#[test]
fn duplicate_binary_paths_preserve_inflight_passes_and_failures() -> anyhow::Result<()> {
    use coding_memory::{compatibility::Identity, state::State};
    for passed in [true, false] {
        let d = tempfile::tempdir()?;
        let mut state = State::open(&d.path().join("state.sqlite"))?;
        let mut identity = Identity {
            binary: d.path().join("first"),
            digest: "9".repeat(64),
            version: "0.162.1".into(),
            stamp: None,
        };
        std::fs::write(&identity.binary, b"same executable")?;
        let second = d.path().join("second");
        std::fs::copy(&identity.binary, &second)?;
        state.observe_runtime(&identity, 10)?;
        let job = state.claim_runtime_check(10)?.unwrap();
        for now in 11..20 {
            identity.binary = if now % 2 == 0 {
                d.path().join("first")
            } else {
                second.clone()
            };
            state.observe_runtime(&identity, now)?;
            assert!(state.claim_runtime_check(now)?.is_none());
        }
        state.finish_runtime_check(&job, passed, 20)?;
        assert_eq!(
            state.observe_runtime(&identity, 21)?.validation,
            if passed { "verified" } else { "failed" }
        );
    }
    Ok(())
}
