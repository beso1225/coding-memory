use anyhow::Result;
use serde_json::{Value, json};
use std::{fs, process::Stdio, time::Duration};
use tempfile::tempdir;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    time::timeout,
};

#[tokio::test(flavor = "current_thread")]
async fn stdio_client_negotiates_and_reads_only_scoped_authoritative_memory() -> Result<()> {
    let d = tempdir()?;
    let vault = d.path().join("vault");
    fs::create_dir(&vault)?;
    fs::write(
        vault.join("queue.md"),
        include_str!("fixtures/vault/queue.md"),
    )?;
    let root = d.path().join("engine");
    fs::create_dir(&root)?;
    let search = coding_memory::search::Search::new(&root, &vault)?;
    search.rebuild()?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_coding-memory"))
        .args([
            "--state-dir",
            root.to_str().unwrap(),
            "mcp",
            "--vault",
            vault.to_str().unwrap(),
            "--project-id",
            "alpha",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let requests = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memory_search","arguments":{"query":"sync_all"}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"memory_read","arguments":{"id":"queue-leases"}}}),
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"memory_search","arguments":{"query":"!!!"}}}),
        json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"memory_search","arguments":{"query":"sync_all"}}}),
        json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"memory_read","arguments":{"id":"queue-leases"}}}),
        json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"memory_search","arguments":{"query":"sync_all"}}}),
    ];
    for request in requests {
        if request["id"] == 6 {
            coding_memory::state::State::open(&root.join("state.sqlite"))?
                .mark_note_pending("queue-leases", "applying")?;
        }
        if request["id"] == 8 {
            coding_memory::state::State::open(&root.join("state.sqlite"))?
                .clear_note_pending("queue-leases")?;
        }
        input.write_all(format!("{request}\n").as_bytes()).await?;
        input.flush().await?;
        if request.get("id").is_none() {
            continue;
        }
        let value = timeout(Duration::from_secs(5), async {
            loop {
                let mut line = String::new();
                anyhow::ensure!(output.read_line(&mut line).await? > 0, "MCP closed");
                let value: Value = serde_json::from_str(&line)?;
                if value["id"] == request["id"] {
                    return Ok::<_, anyhow::Error>(value);
                }
            }
        })
        .await??;
        assert!(value.get("error").is_none(), "{value}");
        match request["id"].as_i64().unwrap() {
            1 => assert!(
                value["result"]["instructions"]
                    .as_str()
                    .unwrap()
                    .contains("English")
            ),
            2 => {
                let tools = value["result"]["tools"].as_array().unwrap();
                assert_eq!(tools.len(), 2);
                let search = tools.iter().find(|t| t["name"] == "memory_search").unwrap();
                assert_eq!(search["inputSchema"]["properties"]["limit"]["minimum"], 1);
                assert_eq!(search["inputSchema"]["properties"]["limit"]["maximum"], 10);
                assert!(
                    tools
                        .iter()
                        .all(|t| t["annotations"]["readOnlyHint"] == true)
                );
            }
            3 | 8 => assert_eq!(
                value["result"]["structuredContent"]["results"][0]["metadata"]["id"],
                "queue-leases"
            ),
            4 => {
                assert!(
                    value["result"]["structuredContent"]["markdown"]
                        .as_str()
                        .unwrap()
                        .contains("State::claim_store")
                );
                assert_eq!(
                    value["result"]["structuredContent"]["evidence_available"][0],
                    false
                );
            }
            5 | 7 => assert_eq!(value["result"]["isError"], true),
            6 => assert!(
                value["result"]["structuredContent"]["results"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            ),
            _ => unreachable!(),
        }
    }
    drop(input);
    timeout(Duration::from_secs(5), child.wait()).await??;
    Ok(())
}
