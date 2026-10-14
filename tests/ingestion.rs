use coding_memory::{
    domain::RunKey,
    state::{Enqueue, State},
};
use tempfile::tempdir;

fn key() -> RunKey {
    RunKey {
        store: "store".into(),
        thread: "thread".into(),
        turn: "turn".into(),
    }
}

#[test]
fn duplicate_signals_and_expired_leases_survive_restart() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("state.sqlite");
    let mut state = State::open(&path)?;
    state.register_project("project", "/repository", 10)?;
    assert_eq!(
        state.enqueue(&key(), "project", "/repository", 11)?,
        Enqueue::Inserted
    );
    assert_eq!(
        state.enqueue(&key(), "project", "/repository", 12)?,
        Enqueue::Duplicate
    );
    let first = state.claim(12, 5)?.unwrap();
    drop(state);
    let mut state = State::open(&path)?;
    assert!(state.claim(16, 5)?.is_none());
    let second = state.claim(17, 5)?.unwrap();
    assert_ne!(first.token, second.token);
    assert!(state.finish(&first, "failed", None, 18).is_err());
    state.finish(&second, "failed", None, 18)?;
    assert_eq!(state.status()?[0].state, "recorded");
    assert!(state.claim(23, 5)?.is_none());
    Ok(())
}

#[test]
fn baseline_and_engine_owned_runs_are_excluded_by_both_routes() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let mut state = State::open(&dir.path().join("state.sqlite"))?;
    state.register_project("project", "/repository", 10)?;
    state.exclude(&key(), "baseline")?;
    assert_eq!(
        state.enqueue(&key(), "project", "/repository", 11)?,
        Enqueue::Excluded
    );
    let mut owned = key();
    owned.store = "engine".into();
    state.own_store("engine")?;
    assert_eq!(
        state.enqueue(&owned, "project", "/repository", 11)?,
        Enqueue::Excluded
    );
    assert!(state.status()?.is_empty());
    Ok(())
}

#[test]
fn worktrees_share_identity_and_evidence_is_immutable() -> anyhow::Result<()> {
    use coding_memory::{domain::Evidence, evidence, project};
    use std::process::Command;
    let d = tempdir()?;
    let repo = d.path().join("repo");
    std::fs::create_dir(&repo)?;
    let git = |args: &[&str]| {
        let r = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    };
    git(&["init"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "--allow-empty",
        "-m",
        "fixture",
    ]);
    let wt = d.path().join("worktree");
    git(&["worktree", "add", "-b", "test", wt.to_str().unwrap()]);
    assert_eq!(project::resolve(&repo)?.id, project::resolve(&wt)?.id);
    let e = Evidence {
        schema_version: 1,
        run: key(),
        project: "project".into(),
        runtime_version: "0.162.1".into(),
        capture_runtime: None,
        outcome: "completed".into(),
        completed_at: 11,
        started_at: 10,
        cwd: None,
        terminal_error: None,
        items: vec![
            serde_json::json!({"id":"native-item","type":"commandExecution","aggregatedOutput":"PASS"}),
        ],
        coverage: vec!["API projection; attachment bytes unavailable".into()],
    };
    let digest = evidence::save(d.path(), &e)?;
    assert_eq!(evidence::save(d.path(), &e)?, digest);
    assert_eq!(evidence::load(d.path(), &digest)?.items, e.items);
    std::fs::write(
        d.path().join("evidence").join(format!("{digest}.json")),
        "corrupt",
    )?;
    assert!(evidence::load(d.path(), &digest).is_err());
    assert!(evidence::load(d.path(), "../escape").is_err());
    Ok(())
}

#[test]
fn observer_cwd_differences_do_not_duplicate_and_late_exclusion_prevents_claim()
-> anyhow::Result<()> {
    let d = tempdir()?;
    let mut state = State::open(&d.path().join("state.sqlite"))?;
    state.register_project("project", "/repo", 1)?;
    state.enqueue(&key(), "project", "/repo/subdir", 2)?;
    assert_eq!(
        state.enqueue(&key(), "project", "/repo", 3)?,
        Enqueue::Duplicate
    );
    state.own_store("store")?;
    assert!(state.claim(4, 10)?.is_none());
    assert_eq!(state.status()?[0].reason.as_deref(), Some("excluded"));
    Ok(())
}

#[test]
fn fixture_decoding_retains_tools_and_distinguishes_readiness() -> anyhow::Result<()> {
    use coding_memory::codex::{self, Capture};
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../experiments/codex_capture/fixtures/cli-0.162.1.json"
    ))?;
    let turn = &fixture["cases"]["later_read"]["read"]["thread"]["turns"][0];
    let key = RunKey {
        store: "store".into(),
        thread: "thread".into(),
        turn: turn["id"].as_str().unwrap().into(),
    };
    let items = turn["items"].as_array().unwrap().clone();
    match codex::decode(&key, "project", "0.162.1", turn, items.clone()) {
        Capture::Ready(e) => {
            assert!(
                e.items
                    .iter()
                    .any(|i| i["aggregatedOutput"] == "synthetic-tool-result")
            );
            assert!(!serde_json::to_string(&e)?.contains("later instruction"));
        }
        other => panic!("{other:?}"),
    }
    let mut pending = turn.clone();
    pending["completedAt"] = serde_json::Value::Null;
    pending["status"] = serde_json::json!("interrupted");
    assert!(matches!(
        codex::decode(&key, "project", "0.162.1", &pending, items.clone()),
        Capture::Pending
    ));
    assert!(matches!(
        codex::decode(&key, "project", "future", turn, items.clone()),
        Capture::Unsupported(_)
    ));
    for outcome in ["failed", "interrupted"] {
        let mut failed = turn.clone();
        failed["status"] = serde_json::json!(outcome);
        assert!(
            matches!(codex::decode(&key,"project","0.162.1",&failed,items.clone()),Capture::Ready(e) if e.outcome==outcome)
        );
    }
    Ok(())
}

#[test]
fn store_scoped_claims_and_expired_finishes_preserve_other_work() -> anyhow::Result<()> {
    let d = tempdir()?;
    let mut state = State::open(&d.path().join("state.sqlite"))?;
    state.register_project("project", "/repo", 1)?;
    state.enqueue(&key(), "project", "/repo", 2)?;
    assert!(state.claim_store(Some("other"), 3, 5)?.is_none());
    let claim = state.claim_store(Some("store"), 3, 5)?.unwrap();
    assert!(
        state
            .finish(&claim, "completed", Some("digest"), 8)
            .is_err()
    );
    let next = state.claim(8, 5)?.unwrap();
    state.hold(&next, "history_missing", 9)?;
    state.retry(&key(), 10)?;
    let fresh = state.claim(10, 5)?.unwrap();
    assert_eq!(fresh.attempts, 1);
    state.finish(&fresh, "completed", Some("digest"), 11)?;
    assert_eq!(state.status()?[0].state, "ready");
    Ok(())
}

#[test]
fn ownership_registered_during_capture_cannot_publish_evidence() -> anyhow::Result<()> {
    let d = tempdir()?;
    let mut state = State::open(&d.path().join("state.sqlite"))?;
    state.register_project("project", "/repo", 1)?;
    state.enqueue(&key(), "project", "/repo", 2)?;
    let claim = state.claim(3, 10)?.unwrap();
    state.own_store("store")?;
    assert!(
        state
            .finish(&claim, "completed", Some("digest"), 4)
            .is_err()
    );
    assert!(state.claim(5, 10)?.is_none());
    assert_eq!(state.status()?[0].state, "held");
    Ok(())
}

#[test]
fn capture_reports_specific_shape_failures_without_source_content() {
    use coding_memory::codex::{Capture, UnsupportedReason as R};
    let key = RunKey {
        store: "fixture".into(),
        thread: "thread".into(),
        turn: "turn".into(),
    };
    let good =
        serde_json::json!({"id":"turn","status":"completed","startedAt":10,"completedAt":11});
    let item = serde_json::json!({"id":"u","type":"userMessage","text":"private content must not enter reasons"});
    for (field, value, reason) in [
        ("id", serde_json::json!("other"), R::TurnIdentity),
        ("status", serde_json::json!("unknown"), R::TurnStatus),
        ("startedAt", serde_json::Value::Null, R::StartedAt),
        ("completedAt", serde_json::json!(9), R::Timestamps),
    ] {
        let mut turn = good.clone();
        turn[field] = value;
        assert!(
            matches!(coding_memory::codex::decode(&key,"alpha","0.162.1",&turn,vec![item.clone()]),Capture::Unsupported(r) if r==reason)
        );
    }
    assert!(matches!(
        coding_memory::codex::decode(&key, "alpha", "0.162.1", &good, vec![item.clone(), item]),
        Capture::Unsupported(R::ItemIdentity)
    ));
    let old = coding_memory::codex::version_diagnostics(
        "0.162.1",
        &serde_json::json!({"cliVersion":"0.162.0-alpha.2","preview":"private preview"}),
    );
    assert_eq!(old.reason, None);
    assert_eq!(
        old.history_creation_version.as_deref(),
        Some("0.162.0-alpha.2")
    );
    let missing = coding_memory::codex::version_diagnostics("0.162.1", &serde_json::json!({}));
    assert_eq!(missing.reason, Some("missing_history_version"));
    let bad = coding_memory::codex::version_diagnostics(
        "0.162.1",
        &serde_json::json!({"cliVersion":"private text\nnot a version"}),
    );
    assert!(bad.history_creation_version.is_none());
    assert!(!serde_json::to_string(&old).unwrap().contains("private"));
}
