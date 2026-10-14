//! Credential-free installed-runtime capture fixture shared by Worker and Actions.
use crate::{
    codex::{self, Capture, Client},
    domain::RunKey,
    ingestion, project,
    state::State,
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::{path::Path, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}
async fn provider() -> Result<(Server, std::net::SocketAddr)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        let mut sequence = 0;
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let exchange = async {
                let mut request = Vec::new();
                let mut buf = [0; 8192];
                let header = loop {
                    let n = stream.read(&mut buf).await?;
                    ensure!(n > 0, "Closed fixture request");
                    request.extend_from_slice(&buf[..n]);
                    ensure!(
                        request.len() <= 2 * 1024 * 1024,
                        "Fixture request too large"
                    );
                    if let Some(i) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let text = std::str::from_utf8(&request[..header])?;
                let is_get = text.starts_with("GET ");
                let length = text
                    .lines()
                    .find_map(|l| {
                        l.split_once(':')
                            .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                ensure!(length <= 2 * 1024 * 1024, "Fixture request too large");
                while request.len() - header < length {
                    let n = stream.read(&mut buf).await?;
                    ensure!(n > 0, "Incomplete fixture request");
                    request.extend_from_slice(&buf[..n]);
                }
                sequence += 1;
                let response = format!("fixture-{sequence}");
                let events = [
                    json!({"type":"response.created","response":{"id":response}}),
                    json!({"type":"response.output_item.done","item":{"type":"message","id":format!("msg-{response}"),"role":"assistant","content":[{"type":"output_text","text":"SYNTHETIC_MEMORY_OK"}]}}),
                    json!({"type":"response.completed","response":{"id":response,"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}),
                ];
                let body = if is_get {
                    "{\"data\":[]}".into()
                } else {
                    events
                        .iter()
                        .map(|e| format!("event: {}\ndata: {}\n\n", e["type"].as_str().unwrap(), e))
                        .collect::<String>()
                };
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",if is_get {"application/json"}else{"text/event-stream"},body.len(),body).as_bytes()).await?;
                Ok::<_, anyhow::Error>(())
            };
            let _ = tokio::time::timeout(Duration::from_secs(10), exchange).await;
        }
    });
    Ok((Server(task), addr))
}
pub async fn run(binary: &Path, root: &Path) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(90), run_fixture(binary, root)).await??;
    Ok(())
}
async fn run_fixture(binary: &Path, root: &Path) -> Result<()> {
    let checks = crate::evidence::private_directory(&root.join("runtime-fixtures"))?;
    let directory = tempfile::Builder::new()
        .prefix("check-")
        .tempdir_in(checks)?;
    let home = directory.path().join("home");
    let work = directory.path().join("project");
    std::fs::create_dir(&home)?;
    std::fs::create_dir(&work)?;
    let git = tokio::process::Command::new("git")
        .args(["-c", "init.templateDir=", "init"])
        .arg(&work)
        .kill_on_drop(true)
        .output()
        .await?;
    ensure!(git.status.success(), "Fixture repository failed");
    let (_server, addr) = provider().await?;
    std::fs::write(
        home.join("config.toml"),
        format!(
            "cli_auth_credentials_store=\"ephemeral\"\napproval_policy=\"never\"\nsandbox_mode=\"read-only\"\nproject_doc_max_bytes=0\nmodel_provider=\"memory_fixture\"\nmodel=\"capture-test\"\nweb_search=\"disabled\"\n[model_providers.memory_fixture]\nname=\"Synthetic local fixture\"\nbase_url=\"http://{addr}/v1\"\nwire_api=\"responses\"\nrequires_openai_auth=false\nrequest_max_retries=0\nstream_max_retries=0\n"
        ),
    )?;
    State::open(&root.join("state.sqlite"))?.own_store(&ingestion::store_id(&home)?)?;
    let mut source = Client::open_fixture(binary, &home, &work).await?;
    let thread = source
        .fixture_call(
            "thread/start",
            json!({"cwd":work,"model":"capture-test","approvalPolicy":"never"}),
        )
        .await?["thread"]["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Fixture thread missing"))?
        .to_owned();
    let project = project::resolve(&work)?;
    for n in 0..2 {
        let instruction =
            format!("Synthetic capture boundary {n}; reply with SYNTHETIC_MEMORY_OK.");
        let turn = source
            .fixture_call(
                "turn/start",
                json!({"threadId":thread,"input":[{"type":"text","text":instruction}]}),
            )
            .await?["turn"]["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Fixture turn missing"))?
            .to_owned();
        source.wait_fixture_turn(&turn).await?;
        let key = RunKey {
            store: ingestion::store_id(&home)?,
            thread: thread.clone(),
            turn,
        };
        let mut reader = Client::open_isolated_reader(binary, &home, &work).await?;
        let Capture::Ready(bundle) = codex::capture(&mut reader, &home, &key, &project.id).await?
        else {
            anyhow::bail!("Fixture capture rejected");
        };
        let users: Vec<_> = bundle
            .items
            .iter()
            .filter(|i| i["type"] == "userMessage")
            .collect();
        ensure!(
            users.len() == 1 && users[0]["content"][0]["text"] == instruction,
            "Fixture turn boundary failed"
        );
        ensure!(
            bundle.items.iter().any(|i| i["type"] == "agentMessage"
                && i["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("SYNTHETIC_MEMORY_OK"))),
            "Fixture response missing"
        );
        let Capture::Ready(repeated) =
            codex::capture(&mut reader, &home, &key, &project.id).await?
        else {
            anyhow::bail!("Repeated fixture read failed");
        };
        ensure!(
            repeated.run == bundle.run && repeated.items == bundle.items,
            "Fixture read changed"
        );
        reader.close().await?;
    }
    source.close().await?;
    Ok(())
}
