use anyhow::Result;
use coding_memory::{
    codex::{Client, turn_cwd},
    domain::RunKey,
    evidence, ingestion, project,
    state::State,
};
use serde_json::json;
use std::{fs, path::Path, process::Command};
use tempfile::tempdir;

fn repository(path: &Path) {
    fs::create_dir_all(path).unwrap();
    assert!(
        Command::new("git")
            .arg("init")
            .arg(path)
            .output()
            .unwrap()
            .status
            .success()
    );
}
fn fake_server(root: &Path) -> std::path::PathBuf {
    let path = root.join("reader.py");
    fs::write(&path,r#"#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
if "--version" in sys.argv:
    print("codex-cli 0.162.1")
    sys.exit(0)
for line in sys.stdin:
    request=json.loads(line)
    if 'id' not in request: continue
    fixture=json.loads((Path(os.environ['CODEX_HOME'])/'fake.json').read_text())
    method=request['method']
    params=request['params']
    if fixture.get('unsupported_method') == method:
        print(json.dumps({'id':request['id'],'error':{'code':-32601,'message':'synthetic'}}),flush=True)
        continue
    if method=='initialize': result={}
    elif method=='thread/list': result={'data':fixture['threads'] if not params.get('archived') else [], 'nextCursor':None}
    elif method=='thread/read': result={'thread':fixture['threads'][0]}
    elif method=='thread/turns/list': result={'data':fixture['turns'], 'nextCursor':None}
    elif method=='thread/items/list': result=fixture.get('item_page', {'data':fixture.get('items',[]), 'nextCursor':fixture.get('cursor')})
    else: raise AssertionError(method)
    print(json.dumps({'id':request['id'],'result':result}),flush=True)
"#).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[tokio::test(flavor = "current_thread")]
async fn reconciled_and_live_runs_share_capture_and_cold_reader_is_bounded() -> Result<()> {
    let d = tempdir()?;
    let repo = d.path().join("repo");
    repository(&repo);
    let home = d.path().join("home");
    fs::create_dir(&home)?;
    let root = evidence::private_directory(&d.path().join("engine"))?;
    let source = home.join("rollout.jsonl");
    fs::write(
        &source,
        format!(
            "{}\n",
            json!({"type":"turn_context","payload":{"turn_id":"turn","cwd":repo}})
        ),
    )?;
    let fixture = json!({"threads":[{"id":"thread","cliVersion":"0.162.0-alpha.2","path":source,"cwd":repo,"parentThreadId":null,"forkedFromId":null}],"turns":[{"id":"turn","rootTurnId":"turn","startedAt":11,"completedAt":12,"status":"completed"}],"items":[{"turnId":"turn","item":{"id":"u","type":"userMessage","content":[{"type":"text","text":"fixture"}]}},{"turnId":"turn","item":{"id":"t","type":"commandExecution","aggregatedOutput":"PASS","exitCode":0}}]});
    fs::write(home.join("fake.json"), serde_json::to_vec(&fixture)?)?;
    let binary = fake_server(d.path());
    let mut client = Client::open(&binary, &home, &repo).await?;
    let p = project::resolve(&repo)?;
    let mut state = State::open(&root.join("state.sqlite"))?;
    state.register_project(&p.id, p.repository.to_str().unwrap(), 10)?;
    assert_eq!(
        ingestion::reconcile(&mut state, &mut client, &home, &repo).await?,
        1
    );
    assert_eq!(
        ingestion::reconcile(&mut state, &mut client, &home, &repo).await?,
        0
    );
    assert!(matches!(
        ingestion::enqueue_signal(
            &mut state,
            &home,
            &json!({"type":"agent-turn-complete","thread-id":"thread","turn-id":"turn","cwd":repo})
        )?,
        coding_memory::state::Enqueue::Duplicate
    ));
    assert!(ingestion::capture_one(&mut state, &mut client, &home, &root).await?);
    let status = state.status()?;
    assert_eq!(status[0].state, "ready");
    let evidence = evidence::load(&root, status[0].evidence.as_deref().unwrap())?;
    let audit = evidence.capture_runtime.as_ref().unwrap();
    assert_eq!(audit.validation, "provisional");
    assert_eq!(audit.reader_version, "0.162.1");
    assert_eq!(audit.binary_sha256.len(), 64);
    assert_eq!(evidence.items[1]["aggregatedOutput"], "PASS");
    let mut changed = fixture.clone();
    changed["items"][1]["item"]["aggregatedOutput"] = json!("later mutable history");
    fs::write(home.join("fake.json"), serde_json::to_vec(&changed)?)?;
    assert_eq!(
        coding_memory::evidence::load(&root, status[0].evidence.as_deref().unwrap())?.items[1]["aggregatedOutput"],
        "PASS"
    );
    changed["cursor"] = json!("repeated");
    fs::write(home.join("fake.json"), serde_json::to_vec(&changed)?)?;
    assert!(
        client
            .pages(
                "thread/items/list",
                json!({"threadId":"thread","turnId":"turn"})
            )
            .await
            .is_err()
    );
    assert!(client.call("thread/resume", json!({})).await.is_err());
    client.close().await?;
    Ok(())
}

#[test]
fn inherited_and_ambiguous_context_cannot_be_attributed_to_current_cwd() -> Result<()> {
    let d = tempdir()?;
    let repo = d.path().join("repo");
    repository(&repo);
    let source = d.path().join("rollout");
    fs::write(
        &source,
        format!(
            "{}\n",
            json!({"type":"turn_context","payload":{"turn_id":"child","cwd":repo}})
        ),
    )?;
    let metadata = json!({"path":source});
    assert!(turn_cwd(d.path(), &metadata, "inherited")?.is_none());
    let other = d.path().join("other");
    repository(&other);
    fs::write(
        &source,
        format!(
            "{}\n{}\n",
            json!({"type":"turn_context","payload":{"turn_id":"child","cwd":repo}}),
            json!({"type":"turn_context","payload":{"turn_id":"child","cwd":other}})
        ),
    )?;
    assert!(turn_cwd(d.path(), &metadata, "child").is_err());
    let key = RunKey {
        store: "s".into(),
        thread: "t".into(),
        turn: "child".into(),
    };
    assert_eq!(key.id().len(), 64);
    Ok(())
}

#[tokio::test]
async fn below_minimum_history_has_a_specific_hold_reason_without_reading_turns() -> Result<()> {
    let d = tempdir()?;
    let repo = d.path().join("repo");
    repository(&repo);
    let home = d.path().join("home");
    fs::create_dir(&home)?;
    fs::write(
        home.join("fake.json"),
        serde_json::to_vec(&json!({"threads":[{"id":"old-app","cliVersion":"0.162.0-alpha.1"}]}))?,
    )?;
    let root = evidence::private_directory(&d.path().join("engine"))?;
    let mut s = State::open(&root.join("state.sqlite"))?;
    let p = project::resolve(&repo)?;
    s.register_project(&p.id, p.repository.to_str().unwrap(), 1)?;
    let key = RunKey {
        store: ingestion::store_id(&home)?,
        thread: "old-app".into(),
        turn: "new-turn".into(),
    };
    s.enqueue(&key, &p.id, repo.to_str().unwrap(), 2)?;
    let binary = fake_server(d.path());
    let output = Command::new(env!("CARGO_BIN_EXE_coding-memory"))
        .arg("--state-dir")
        .arg(&root)
        .arg("capture-diagnose")
        .arg("--codex-binary")
        .arg(&binary)
        .arg("--codex-home")
        .arg(&home)
        .arg("--project")
        .arg(&repo)
        .args(["--thread", "old-app"])
        .output()?;
    assert!(output.status.success());
    let diagnostics: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(diagnostics["history_creation_version"], "0.162.0-alpha.1");
    assert_eq!(diagnostics["reader_version"], "0.162.1");
    assert_eq!(diagnostics["history_version_supported"], false);
    assert_eq!(s.status()?[0].state, "queued");
    assert!(!root.join("generation").exists());
    let mut client = Client::open(&fake_server(d.path()), &home, &repo).await?;
    assert!(ingestion::capture_one(&mut s, &mut client, &home, &root).await?);
    let row = &s.status()?[0];
    assert_eq!(row.state, "held");
    assert_eq!(row.reason.as_deref(), Some("unsupported_history_version"));
    assert!(row.evidence.is_none());
    client.close().await?;
    Ok(())
}

#[tokio::test]
async fn paging_and_source_failures_persist_distinct_safe_reasons() -> Result<()> {
    for reason in [
        "missing_source_path",
        "missing_turn_context",
        "unsupported_history_method",
        "invalid_history_page_data",
        "invalid_history_page_cursor",
    ] {
        let d = tempdir()?;
        let repo = d.path().join("repo");
        repository(&repo);
        let home = d.path().join("home");
        fs::create_dir(&home)?;
        let source = home.join("rollout.jsonl");
        fs::write(
            &source,
            format!(
                "{}\n",
                json!({"type":"turn_context","payload":{"turn_id":"turn","cwd":repo}})
            ),
        )?;
        let mut fixture = json!({"threads":[{"id":"thread","cliVersion":"0.162.0-alpha.17.2","path":source}],"turns":[{"id":"turn","startedAt":11,"completedAt":12,"status":"completed"}],"items":[]});
        match reason {
            "missing_source_path" => fixture["threads"][0]["path"] = json!(null),
            "missing_turn_context" => fs::write(&source, "")?,
            "unsupported_history_method" => {
                fixture["unsupported_method"] = json!("thread/turns/list")
            }
            "invalid_history_page_data" => {
                fixture["item_page"] = json!({"data":null,"nextCursor":null})
            }
            "invalid_history_page_cursor" => fixture["item_page"] = json!({"data":[]}),
            _ => unreachable!(),
        }
        fs::write(home.join("fake.json"), serde_json::to_vec(&fixture)?)?;
        let root = evidence::private_directory(&d.path().join("engine"))?;
        let mut state = State::open(&root.join("state.sqlite"))?;
        let p = project::resolve(&repo)?;
        state.register_project(&p.id, p.repository.to_str().unwrap(), 1)?;
        let key = RunKey {
            store: ingestion::store_id(&home)?,
            thread: "thread".into(),
            turn: "turn".into(),
        };
        state.enqueue(&key, &p.id, repo.to_str().unwrap(), 2)?;
        let mut client = Client::open(&fake_server(d.path()), &home, &repo).await?;
        ingestion::capture_one(&mut state, &mut client, &home, &root).await?;
        assert_eq!(state.status()?[0].reason.as_deref(), Some(reason));
        assert!(state.status()?[0].evidence.is_none());
        client.close().await?;
    }
    Ok(())
}

#[tokio::test]
async fn failed_runtime_verification_blocks_capture_without_claiming_work() -> Result<()> {
    let d = tempdir()?;
    let repo = d.path().join("repo");
    repository(&repo);
    let home = d.path().join("home");
    fs::create_dir(&home)?;
    fs::write(home.join("fake.json"), b"{}")?;
    let root = evidence::private_directory(&d.path().join("engine"))?;
    let mut state = State::open(&root.join("state.sqlite"))?;
    let project = project::resolve(&repo)?;
    state.register_project(&project.id, project.repository.to_str().unwrap(), 1)?;
    let key = RunKey {
        store: ingestion::store_id(&home)?,
        thread: "fixture".into(),
        turn: "turn".into(),
    };
    state.enqueue(&key, &project.id, repo.to_str().unwrap(), 2)?;
    let mut client = Client::open(&fake_server(d.path()), &home, &repo).await?;
    let identity = client.identity().clone();
    let now = ingestion::now();
    state.observe_runtime(&identity, now)?;
    let job = state.claim_runtime_check(now)?.unwrap();
    state.finish_runtime_check(&job, false, now + 1)?;
    assert!(
        ingestion::capture_one(&mut state, &mut client, &home, &root)
            .await
            .unwrap_err()
            .to_string()
            .contains("runtime_validation_failed")
    );
    assert_eq!(state.status()?[0].state, "queued");
    client.close().await?;
    Ok(())
}
