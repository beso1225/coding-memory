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
    if method=='initialize': result={}
    elif method=='thread/list': result={'data':fixture['threads'] if not params.get('archived') else [], 'nextCursor':None}
    elif method=='thread/read': result={'thread':fixture['threads'][0]}
    elif method=='thread/turns/list': result={'data':fixture['turns'], 'nextCursor':None}
    elif method=='thread/items/list': result={'data':fixture['items'], 'nextCursor':fixture.get('cursor')}
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
    let fixture = json!({"threads":[{"id":"thread","cliVersion":"0.162.1","path":source,"cwd":repo,"parentThreadId":null,"forkedFromId":null}],"turns":[{"id":"turn","rootTurnId":"turn","startedAt":11,"completedAt":12,"status":"completed"}],"items":[{"turnId":"turn","item":{"id":"u","type":"userMessage","content":[{"type":"text","text":"fixture"}]}},{"turnId":"turn","item":{"id":"t","type":"commandExecution","aggregatedOutput":"PASS","exitCode":0}}]});
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
