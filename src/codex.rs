//! Read-only, paginated Codex app-server client. No start/resume methods are exposed.
use crate::{
    domain::{Evidence, RunKey},
    evidence::read_bounded,
    project,
};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

pub const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_PAGES: usize = 100;
#[derive(Debug)]
pub enum ReadFailure {
    Missing,
    Unsupported,
    OverLimit,
}
impl std::fmt::Display for ReadFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ReadFailure {}
pub struct Client {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
}
impl Client {
    pub async fn open(binary: &Path, home: &Path, cwd: &Path) -> Result<Self> {
        let version = crate::generation::version(binary, home)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e.reason()))?;
        ensure!(supported(&version), "Unsupported Codex reader executable");
        let mut child = Command::new(binary)
            .args([
                "-c",
                "notify=[]",
                "-c",
                "analytics.enabled=false",
                "--disable",
                "hooks",
                "--disable",
                "plugins",
                "app-server",
            ])
            .env("CODEX_HOME", home)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let input = child.stdin.take().expect("piped stdin");
        let output = BufReader::new(child.stdout.take().expect("piped stdout"));
        let mut client = Self {
            child,
            input,
            output,
            next: 0,
        };
        client.call("initialize",json!({"clientInfo":{"name":"coding_memory_reader","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})).await?;
        client
            .send(json!({"method":"initialized","params":{}}))
            .await?;
        Ok(client)
    }
    async fn send(&mut self, value: Value) -> Result<()> {
        self.input.write_all(&serde_json::to_vec(&value)?).await?;
        self.input.write_all(b"\n").await?;
        self.input.flush().await?;
        Ok(())
    }
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        ensure!(
            [
                "initialize",
                "thread/read",
                "thread/list",
                "thread/turns/list",
                "thread/items/list"
            ]
            .contains(&method),
            "Read-only method allowlist"
        );
        self.next += 1;
        let id = self.next;
        timeout(Duration::from_secs(30), async {
            self.send(json!({"id":id,"method":method,"params":params}))
                .await?;
            let mut consumed = 0;
            loop {
                let mut line = Vec::new();
                let n = (&mut self.output)
                    .take((MAX_BYTES + 1) as u64)
                    .read_until(b'\n', &mut line)
                    .await?;
                ensure!(n > 0, "Codex reader closed stdout");
                consumed += n;
                if consumed > MAX_BYTES {
                    return Err(ReadFailure::OverLimit.into());
                }
                let value: Value = serde_json::from_slice(&line)?;
                if value.get("id") == Some(&json!(id)) {
                    if let Some(e) = value.get("error") {
                        let message = e["message"].as_str().unwrap_or("").to_lowercase();
                        if message.contains("not found") || message.contains("no rollout") {
                            return Err(ReadFailure::Missing.into());
                        }
                        if e["code"] == -32601 {
                            return Err(ReadFailure::Unsupported.into());
                        }
                        bail!("Codex RPC error: {}", e.get("code").unwrap_or(&Value::Null));
                    }
                    return value
                        .get("result")
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("Missing RPC result"));
                }
                ensure!(
                    value.get("id").is_none(),
                    "Unexpected server request or reply"
                );
                ensure!(
                    value.get("method").and_then(Value::as_str) != Some("thread/started"),
                    "Cold reader unexpectedly started a thread"
                );
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("Codex reader timeout"))?
    }
    pub async fn pages(&mut self, method: &str, mut params: Value) -> Result<Vec<Value>> {
        let mut data = Vec::new();
        let mut bytes = 0;
        let mut cursors = HashSet::new();
        for _ in 0..MAX_PAGES {
            let page = self.call(method, params.clone()).await?;
            bytes += serde_json::to_vec(&page)?.len();
            if bytes > MAX_BYTES {
                return Err(ReadFailure::OverLimit.into());
            }
            data.extend(
                page.get("data")
                    .and_then(Value::as_array)
                    .ok_or(ReadFailure::Unsupported)?
                    .iter()
                    .cloned(),
            );
            match page.get("nextCursor") {
                Some(Value::Null) => return Ok(data),
                Some(Value::String(cursor)) => {
                    ensure!(cursors.insert(cursor.clone()), "Repeated page cursor");
                    params["cursor"] = json!(cursor);
                }
                _ => return Err(ReadFailure::Unsupported.into()),
            }
        }
        Err(ReadFailure::OverLimit.into())
    }
    pub async fn turns(&mut self, thread: &str) -> Result<Vec<Value>> {
        self.pages(
            "thread/turns/list",
            json!({"threadId":thread,"limit":100,"itemsView":"notLoaded","sortDirection":"asc"}),
        )
        .await
    }
    pub async fn threads(&mut self, roots: &[PathBuf]) -> Result<Vec<Value>> {
        let mut result = Vec::new();
        for archived in [false, true] {
            result.extend(self.pages("thread/list",json!({"cwd":roots,"limit":100,"sourceKinds":["cli","exec","vscode","appServer"],"archived":archived,"useStateDbOnly":true})).await?);
        }
        Ok(result)
    }
    pub async fn close(mut self) -> Result<()> {
        self.child.kill().await?;
        Ok(())
    }
}
#[derive(Debug)]
pub enum Capture {
    Ready(Box<Evidence>),
    Pending,
    Missing,
    Unsupported,
    OverLimit,
    Excluded,
    WrongProject,
}

pub fn supported(version: &str) -> bool {
    matches!(version, "0.162.1" | "0.162.0-alpha.17.2")
}
/// Read only turn_context identity/cwd from the bounded local source artifact.
/// An absent context is never attributed from a thread's current cwd.
pub fn turn_cwd(home: &Path, metadata: &Value, turn: &str) -> Result<Option<PathBuf>> {
    let path = metadata
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Missing source artifact"))?;
    let source = Path::new(path).canonicalize().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::Error::from(ReadFailure::Missing)
        } else {
            e.into()
        }
    })?;
    ensure!(
        source.starts_with(home.canonicalize()?),
        "Source artifact outside registered runtime store"
    );
    if std::fs::metadata(&source)?.len() > 64 * 1024 * 1024 {
        return Err(ReadFailure::OverLimit.into());
    }
    let bytes = read_bounded(&source, 64 * 1024 * 1024)?;
    let mut cwd = None;
    for line in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        // A concurrently appended final record may be incomplete. Only complete
        // records can establish the selected turn's association.
        let record: Value = match serde_json::from_slice(line) {
            Ok(v) => v,
            Err(e) if e.is_eof() => continue,
            Err(e) => return Err(e.into()),
        };
        if record["type"] == "turn_context" && record["payload"]["turn_id"] == turn {
            let next = PathBuf::from(
                record["payload"]["cwd"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing turn cwd"))?,
            )
            .canonicalize()?;
            ensure!(
                cwd.as_ref().is_none_or(|p| *p == next),
                "Ambiguous turn project"
            );
            cwd = Some(next);
        }
    }
    Ok(cwd)
}
pub fn decode(
    key: &RunKey,
    project: &str,
    version: &str,
    turn: &Value,
    items: Vec<Value>,
) -> Capture {
    if !supported(version) || turn["id"] != key.turn {
        return Capture::Unsupported;
    }
    let outcome = match turn["status"].as_str() {
        Some("completed") => "completed",
        Some("failed") => "failed",
        Some("interrupted") => "interrupted",
        Some("inProgress") => return Capture::Pending,
        _ => return Capture::Unsupported,
    };
    let Some(completed) = turn["completedAt"].as_i64() else {
        return Capture::Pending;
    };
    let Some(started) = turn["startedAt"].as_i64() else {
        return Capture::Unsupported;
    };
    if completed < started {
        return Capture::Unsupported;
    }
    let mut ids = HashSet::new();
    if items.iter().any(|i| {
        i["id"].as_str().is_none_or(|id| !ids.insert(id.to_owned())) || i["type"].as_str().is_none()
    }) {
        return Capture::Unsupported;
    }
    if !items.iter().any(|i| i["type"] == "userMessage")
        || turn["rootTurnId"]
            .as_str()
            .is_some_and(|root| root != key.turn)
    {
        return Capture::Excluded;
    }
    if serde_json::to_vec(&items).map_or(true, |v| v.len() > MAX_BYTES) {
        return Capture::OverLimit;
    }
    Capture::Ready(Box::new(Evidence {
        schema_version: 1,
        run: key.clone(),
        project: project.into(),
        runtime_version: version.into(),
        outcome: outcome.into(),
        completed_at: completed,
        started_at: started,
        cwd: None,
        terminal_error: turn.get("error").filter(|e| !e.is_null()).cloned(),
        items,
        coverage: vec![
            "Persisted app-server item projection, not original transcript".into(),
            "Runtime may truncate tool output; attachment bytes and hidden reasoning unavailable"
                .into(),
            "Only selected own turn; inherited and subsequent turns excluded".into(),
        ],
    }))
}
pub async fn capture(
    client: &mut Client,
    home: &Path,
    key: &RunKey,
    project_id: &str,
) -> Result<Capture> {
    let metadata = client
        .call(
            "thread/read",
            json!({"threadId":key.thread,"includeTurns":false}),
        )
        .await?;
    let thread = &metadata["thread"];
    let Some(version) = thread["cliVersion"].as_str() else {
        return Ok(Capture::Unsupported);
    };
    if !supported(version) {
        return Ok(Capture::Unsupported);
    }
    if !thread["parentThreadId"].is_null()
        || thread["source"]
            .as_str()
            .is_some_and(|s| s.starts_with("subAgent"))
    {
        return Ok(Capture::Excluded);
    }
    let turns = client.turns(&key.thread).await?;
    let Some(turn) = turns.iter().find(|t| t["id"] == key.turn) else {
        return Ok(Capture::Missing);
    };
    if turn["completedAt"].is_null() {
        return Ok(Capture::Pending);
    }
    // Fork prefix turns have no turn_context in the child artifact. Reject them.
    let Some(cwd) = turn_cwd(home, thread, &key.turn)? else {
        return Ok(if thread["forkedFromId"].is_null() {
            Capture::Unsupported
        } else {
            Capture::Excluded
        });
    };
    if project::resolve(&cwd)?.id != project_id {
        return Ok(Capture::WrongProject);
    }
    let entries = client
        .pages(
            "thread/items/list",
            json!({"threadId":key.thread,"turnId":key.turn,"limit":100,"sortDirection":"asc"}),
        )
        .await?;
    let mut items = Vec::new();
    for entry in entries {
        ensure!(entry["turnId"] == key.turn, "Item belongs to another turn");
        items.push(
            entry
                .get("item")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Missing item projection"))?,
        );
    }
    let mut decoded = decode(key, project_id, version, turn, items);
    if let Capture::Ready(bundle) = &mut decoded {
        bundle.cwd = Some(
            cwd.to_str()
                .ok_or_else(|| anyhow::anyhow!("Non-UTF8 turn cwd"))?
                .into(),
        );
    }
    Ok(decoded)
}
