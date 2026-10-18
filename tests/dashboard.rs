use anyhow::Result;
use coding_memory::{state::State, worker::Ownership};
use rusqlite::Connection;
use std::{fs, process::Command};
use tempfile::tempdir;

fn render(root: &std::path::Path) -> Result<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_coding-memory"))
        .arg("--state-dir")
        .arg(root)
        .arg("dashboard")
        .output()?;
    assert!(output.status.success(), "{:?}", output);
    Ok(String::from_utf8(output.stdout)?)
}

#[test]
fn dashboard_is_read_only_escapes_metadata_and_distinguishes_outcomes() -> Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("state.sqlite");
    let mut state = State::open(&path)?;
    let project = "alpha<script>&\"'";
    state.register_project(project, "/repo", 1)?;
    let db = Connection::open(&path)?;
    for (i, status) in [
        "queued",
        "held",
        "exhausted",
        "succeeded_empty",
        "succeeded",
    ]
    .iter()
    .enumerate()
    {
        db.execute("INSERT INTO runs(store,thread,turn,project,cwd,state,available_at,outcome) VALUES('s',?1,'t',?2,'/repo','ready',1,'completed')", rusqlite::params![i.to_string(),project])?;
        db.execute("INSERT INTO extraction_jobs(store,thread,turn,project,evidence,state,reason,available_at,finished_at,provenance) VALUES('s',?1,'t',?2,'secret evidence',?3,?4,1,100,'secret prompt')", rusqlite::params![i.to_string(),project,status,if *status == "held" {"generation_auth_unavailable"} else {"secret stderr"}])?;
    }
    db.execute("INSERT INTO consolidation_jobs(project,state,reason,created_at,lease_until) VALUES(?1,'conflict','revision_conflict',1,2)", [project])?;
    db.execute("INSERT INTO consolidation_jobs(project,state,created_at,lease_until) VALUES(?1,'applied',1,2)", [project])?;
    db.execute("INSERT INTO vault_changes VALUES(2,'memory<&>','secret path',NULL,'hash','secret conversation',1)", [])?;
    db.execute("INSERT INTO runs(store,thread,turn,project,cwd,state,available_at,outcome) VALUES('s','failed','t',?1,'/repo','recorded',1,'failed')", [project])?;
    state.save_worker_report(&coding_memory::worker::Report {
        started_at: 100,
        completed_at: Some(110),
        capture_attempts: 8,
        failures: vec![coding_memory::worker::Failure {
            project: Some(project.into()),
            stage: "capture_failed".into(),
        }],
        ..Default::default()
    })?;
    let database_before = fs::read(&path)?;
    let before: String =
        db.query_row("SELECT group_concat(state) FROM extraction_jobs", [], |r| {
            r.get(0)
        })?;
    let owner = Ownership::acquire(dir.path())?;
    let html = render(dir.path())?;
    assert!(html.contains("Worker lock: active"));
    drop(owner);
    assert!(render(dir.path())?.contains("Worker lock: inactive"));
    assert!(html.contains("alpha&lt;script&gt;&amp;&quot;&#39;"));
    assert!(!html.contains(project));
    for label in [
        "queued",
        "held",
        "exhausted",
        "succeeded_empty",
        "succeeded",
        "conflict",
        "No candidates",
        "memory&lt;&amp;&gt;",
        "Snapshot obtained",
        "Regenerate",
        "Authentication unavailable",
        "Revision conflict",
        "Capture complete; available for extraction",
        "recorded (failed)",
        "Attempts: capture 8",
        "Reported failures: 1",
        "A completed cycle can include failures",
    ] {
        assert!(html.contains(label), "Missing {label}");
    }
    assert!(!html.contains("secret"));
    let after: String =
        db.query_row("SELECT group_concat(state) FROM extraction_jobs", [], |r| {
            r.get(0)
        })?;
    assert_eq!(before, after);
    assert_eq!(database_before, fs::read(path)?);
    assert!(!dir.path().join("evidence").exists());
    Ok(())
}

#[test]
fn missing_section_is_unavailable_while_empty_sections_are_zero() -> Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("state.sqlite");
    let state = State::open(&path)?;
    let db = Connection::open(path)?;
    db.execute("DROP TABLE vault_changes", [])?;
    let html = render(dir.path())?;
    assert!(html.contains("0 records (data available)."));
    assert!(html.contains(
        "<h2>Published memory records</h2><p class=attention data-unavailable>Unavailable"
    ));
    assert!(!dir.path().join("worker.lock").exists());
    drop(state);
    Ok(())
}

#[test]
fn unavailable_state_is_not_reported_as_zero_or_initialized() -> Result<()> {
    let dir = tempdir()?;
    let missing = dir.path().join("missing<&\"'");
    let html = render(&missing)?;
    assert!(html.contains("Unavailable"));
    assert!(html.contains("State directory:"));
    assert!(html.contains("missing&lt;&amp;&quot;&#39;"));
    assert!(!missing.exists());
    assert!(!dir.path().join("state.sqlite").exists());
    fs::write(dir.path().join("state.sqlite"), b"invalid database secret")?;
    let html = render(dir.path())?;
    assert!(html.contains("Unavailable"));
    assert!(!html.contains("invalid database secret"));
    Ok(())
}
