use anyhow::Result;
use coding_memory::{
    domain::RunKey,
    notes,
    search::{Scope, Search},
    state::State,
};
use std::fs;
use tempfile::tempdir;

#[test]
fn markdown_roundtrip_requires_explicit_scope_and_preserves_support() -> Result<()> {
    let source = include_str!("fixtures/vault/queue.md");
    let note = notes::parse(source)?;
    assert_eq!(note.metadata.id, "queue-leases");
    assert_eq!(
        notes::parse(&notes::render(&note)?)?.metadata,
        note.metadata
    );
    assert!(notes::parse(&source.replace("project_id: alpha", "project_id: ''")).is_err());
    assert!(notes::parse(&source.replace("schema_version: 1", "schema_version: 99")).is_err());
    Ok(())
}

#[test]
fn rebuild_is_derived_and_scope_query_and_freshness_are_enforced() -> Result<()> {
    let d = tempdir()?;
    let vault = d.path().join("vault");
    fs::create_dir(&vault)?;
    let source = include_str!("fixtures/vault/queue.md");
    fs::write(vault.join("queue.md"), source)?;
    fs::write(
        vault.join("other.md"),
        source
            .replace("queue-leases", "other-note")
            .replace("project_id: alpha", "project_id: beta"),
    )?;
    fs::write(
        vault.join("old.md"),
        source.replace("queue-leases", "old-note").replace(
            "status: active",
            "status: superseded\nsuccessor: queue-leases",
        ),
    )?;
    let mut state = State::open(&d.path().join("state.sqlite"))?;
    state.register_project("alpha", "/repo", 1)?;
    let key = RunKey {
        store: "s".into(),
        thread: "t".into(),
        turn: "r".into(),
    };
    state.enqueue(&key, "alpha", "/repo", 2)?;
    let search = Search::new(d.path(), &vault)?;
    assert_eq!(search.rebuild()?.indexed, 3);
    assert_eq!(state.status()?.len(), 1);
    let claim = state.claim(3, 100)?.unwrap();
    let scope = Scope::new("alpha");
    for query in ["State::claim_store", "sync_all", "src/state.rs"] {
        let response = search.search(query, &scope, 5)?;
        assert_eq!(response.results.len(), 1, "{query}");
        assert_eq!(response.results[0].metadata.id, "queue-leases");
    }
    state.finish(&claim, "failed", None, 4)?;
    assert_eq!(search.search("sync_all", &scope, 5)?.results.len(), 1);
    assert!(search.search("   ", &scope, 5).is_err());
    assert!(search.search("!!!", &scope, 5).is_err());
    assert!(
        search
            .search("sync_all OR anything", &scope, 5)?
            .results
            .is_empty()
    );
    assert!(search.read("other-note", &scope).is_err());
    let mut broad = scope.clone();
    broad.other_projects.push("beta".into());
    broad.include_historical = true;
    assert_eq!(search.search("sync_all", &broad, 5)?.results.len(), 3);
    state.mark_note_pending("queue-leases", "applying")?;
    assert!(search.search("sync_all", &scope, 5)?.results.is_empty());
    assert!(search.read("queue-leases", &scope).is_err());
    state.clear_note_pending("queue-leases")?;
    fs::write(
        vault.join("queue.md"),
        source.replace("Recover expired", "Changed expired"),
    )?;
    assert!(search.search("sync_all", &scope, 5)?.results.is_empty());
    assert!(search.read("queue-leases", &scope).is_err());
    search.rebuild()?;
    assert_eq!(
        search.read("queue-leases", &scope)?.metadata.title,
        "Changed expired capture leases"
    );
    Ok(())
}

#[test]
fn global_notes_duplicates_and_symlinks_are_handled_without_scope_fallback() -> Result<()> {
    let d = tempdir()?;
    let vault = d.path().join("vault");
    fs::create_dir(&vault)?;
    let source = include_str!("fixtures/vault/queue.md");
    let global = source
        .replace("queue-leases", "global-note")
        .replace("kind: project\n  project_id: alpha", "kind: global");
    fs::write(vault.join("global.md"), global)?;
    fs::write(vault.join("one.md"), source)?;
    fs::write(vault.join("two.md"), source)?;
    let outside = d.path().join("outside.md");
    fs::write(&outside, source.replace("queue-leases", "escaped-note"))?;
    std::os::unix::fs::symlink(&outside, vault.join("escaped.md"))?;
    let search = Search::new(d.path(), &vault)?;
    let report = search.rebuild()?;
    assert_eq!(report.indexed, 1);
    assert_eq!(report.invalid.len(), 2);
    let response = search.search("sync_all", &Scope::new("beta"), 5)?;
    assert_eq!(response.results[0].metadata.id, "global-note");
    assert!(search.read("queue-leases", &Scope::new("alpha")).is_err());
    assert!(search.read("../outside", &Scope::new("alpha")).is_err());
    assert!(search.search("sync_all", &Scope::new("alpha"), 0).is_err());
    assert!(
        search
            .search(&"x".repeat(1025), &Scope::new("alpha"), 5)
            .is_err()
    );
    fs::remove_file(vault.join("global.md"))?;
    assert!(
        search
            .search("sync_all", &Scope::new("beta"), 5)?
            .results
            .is_empty()
    );
    Ok(())
}

#[test]
fn local_evidence_availability_checks_hash_and_native_item_locator() -> Result<()> {
    use coding_memory::{domain::Evidence, evidence};
    let d = tempdir()?;
    let vault = d.path().join("vault");
    fs::create_dir(&vault)?;
    let bundle = Evidence {
        schema_version: 1,
        run: RunKey {
            store: "s".into(),
            thread: "t".into(),
            turn: "r".into(),
        },
        project: "alpha".into(),
        runtime_version: "fixture".into(),
        outcome: "completed".into(),
        completed_at: 2,
        started_at: 1,
        cwd: None,
        terminal_error: None,
        items: vec![serde_json::json!({"id":"fixture-E1","type":"agentMessage","text":"PASS"})],
        coverage: vec![],
    };
    let digest = evidence::save(d.path(), &bundle)?;
    let source = include_str!("fixtures/vault/queue.md").replace(&"a".repeat(64), &digest);
    fs::write(vault.join("queue.md"), source)?;
    let search = Search::new(d.path(), &vault)?;
    search.rebuild()?;
    assert_eq!(
        search
            .read("queue-leases", &Scope::new("alpha"))?
            .evidence_available,
        vec![true]
    );
    fs::write(
        d.path().join("evidence").join(format!("{digest}.json")),
        "corrupt",
    )?;
    assert_eq!(
        search
            .read("queue-leases", &Scope::new("alpha"))?
            .evidence_available,
        vec![false]
    );
    Ok(())
}
