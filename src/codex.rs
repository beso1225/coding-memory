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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedReason {
    MissingHistoryVersion,
    HistoryVersion,
    UnsupportedMethod,
    PageData,
    PageCursor,
    TurnIdentity,
    TurnStatus,
    StartedAt,
    Timestamps,
    ItemIdentity,
    TurnContext,
    SourcePath,
    TurnCwd,
    ConflictingTurnContext,
    ItemProjection,
    ItemTurn,
}
impl UnsupportedReason {
    pub fn reason(self) -> &'static str {
        match self {
            Self::MissingHistoryVersion => "missing_history_version",
            Self::HistoryVersion => "unsupported_history_version",
            Self::UnsupportedMethod => "unsupported_history_method",
            Self::PageData => "invalid_history_page_data",
            Self::PageCursor => "invalid_history_page_cursor",
            Self::TurnIdentity => "invalid_turn_identity",
            Self::TurnStatus => "unsupported_turn_status",
            Self::StartedAt => "missing_turn_start",
            Self::Timestamps => "invalid_turn_timestamps",
            Self::ItemIdentity => "invalid_item_identity",
            Self::TurnContext => "missing_turn_context",
            Self::SourcePath => "missing_source_path",
            Self::TurnCwd => "missing_turn_cwd",
            Self::ConflictingTurnContext => "conflicting_turn_context",
            Self::ItemProjection => "missing_item_projection",
            Self::ItemTurn => "item_turn_mismatch",
        }
    }
}
/// Thread creation version is independent of the selected reader executable.
pub fn history_version(thread: &Value) -> std::result::Result<&str, UnsupportedReason> {
    let version = thread["cliVersion"]
        .as_str()
        .ok_or(UnsupportedReason::MissingHistoryVersion)?;
    if history_supported(version) {
        Ok(version)
    } else {
        Err(UnsupportedReason::HistoryVersion)
    }
}
#[derive(Debug, serde::Serialize)]
pub struct HistoryDiagnostics {
    pub reader_version: String,
    pub history_creation_version: Option<String>,
    pub history_version_supported: bool,
    pub reason: Option<&'static str>,
}
pub fn version_diagnostics(reader_version: &str, thread: &Value) -> HistoryDiagnostics {
    let version = thread["cliVersion"].as_str().filter(|v| {
        v.len() <= 128
            && !v.is_empty()
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
    });
    let result = history_version(thread);
    HistoryDiagnostics {
        reader_version: reader_version.into(),
        history_creation_version: version.map(str::to_owned),
        history_version_supported: result.is_ok(),
        reason: result.err().map(UnsupportedReason::reason),
    }
}
#[derive(Debug)]
pub enum ReadFailure {
    Missing,
    Unsupported(UnsupportedReason),
    OverLimit,
}
impl std::fmt::Display for ReadFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ReadFailure {}
pub struct Client {
    group: crate::generation::ProcessGroup,
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
    version: String,
    binary: PathBuf,
    fixture: bool,
    identity: crate::compatibility::Identity,
    fixture_terminals: std::collections::HashMap<String, String>,
}
impl Client {
    pub async fn open(binary: &Path, home: &Path, cwd: &Path) -> Result<Self> {
        Self::open_mode(binary, home, cwd, false, false).await
    }
    async fn open_mode(
        binary: &Path,
        home: &Path,
        cwd: &Path,
        isolated: bool,
        fixture: bool,
    ) -> Result<Self> {
        let identity = crate::compatibility::identify(binary, home).await?;
        let version = identity.version.clone();
        let binary = identity.binary.as_path();
        let mut command = Command::new(binary);
        command
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
            .env_remove("OPENAI_API_KEY")
            .env_remove("CODEX_API_KEY")
            .env_remove("OPENAI_BASE_URL")
            .env_remove("CODEX_ACCESS_TOKEN")
            .env_remove("OPENAI_IDENTITY_TOKEN_FILE")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if isolated {
            command
                .env_clear()
                .env("CODEX_HOME", home)
                .env("HOME", home)
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env("LANG", "C.UTF-8");
        }
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn()?;
        let group = crate::generation::ProcessGroup::new(
            child
                .id()
                .ok_or_else(|| anyhow::anyhow!("Missing reader process ID"))?,
        );
        let input = child.stdin.take().expect("piped stdin");
        let output = BufReader::new(child.stdout.take().expect("piped stdout"));
        let mut client = Self {
            group,
            child,
            input,
            output,
            next: 0,
            version,
            binary: binary.into(),
            fixture,
            identity,
            fixture_terminals: std::collections::HashMap::new(),
        };
        client.call("initialize",json!({"clientInfo":{"name":"coding_memory_reader","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})).await?;
        client
            .send(json!({"method":"initialized","params":{}}))
            .await?;
        Ok(client)
    }
    /// Metadata only: no turn/item read, resume, capture, generation or queue mutation.
    pub async fn diagnose_thread(&mut self, thread: &str) -> Result<HistoryDiagnostics> {
        ensure!(
            !thread.is_empty() && thread.len() <= 256,
            "Invalid diagnostic thread ID"
        );
        let response = self
            .call(
                "thread/read",
                json!({"threadId":thread,"includeTurns":false}),
            )
            .await?;
        Ok(version_diagnostics(&self.version, &response["thread"]))
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
        self.exchange(method, params).await
    }
    pub fn reader_version(&self) -> &str {
        &self.version
    }
    pub fn binary(&self) -> &Path {
        &self.binary
    }
    pub fn identity(&self) -> &crate::compatibility::Identity {
        &self.identity
    }
    pub(crate) async fn fixture_call(&mut self, method: &str, params: Value) -> Result<Value> {
        ensure!(
            self.fixture && ["thread/start", "turn/start"].contains(&method),
            "Fixture-only method"
        );
        self.exchange(method, params).await
    }
    pub(crate) async fn open_fixture(binary: &Path, home: &Path, cwd: &Path) -> Result<Self> {
        Self::open_mode(binary, home, cwd, true, true).await
    }
    pub(crate) async fn open_isolated_reader(
        binary: &Path,
        home: &Path,
        cwd: &Path,
    ) -> Result<Self> {
        Self::open_mode(binary, home, cwd, true, false).await
    }
    fn fixture_notification(&mut self, value: &Value) {
        if self.fixture
            && value["method"] == "turn/completed"
            && let (Some(id), Some(status)) = (
                value["params"]["turn"]["id"].as_str(),
                value["params"]["turn"]["status"].as_str(),
            )
            && id.len() <= 256
            && status.len() <= 32
            && self.fixture_terminals.len() < 4
        {
            self.fixture_terminals.insert(id.into(), status.into());
        }
    }
    pub(crate) async fn wait_fixture_turn(&mut self, turn: &str) -> Result<()> {
        ensure!(self.fixture, "Fixture-only lifecycle wait");
        timeout(Duration::from_secs(30), async {
            let mut consumed = 0;
            while !self.fixture_terminals.contains_key(turn) {
                let mut line = Vec::new();
                let n = (&mut self.output)
                    .take((MAX_BYTES + 1) as u64)
                    .read_until(b'\n', &mut line)
                    .await?;
                consumed += n;
                ensure!(
                    n > 0 && consumed <= MAX_BYTES,
                    "Fixture notification stream unavailable"
                );
                let value: Value = serde_json::from_slice(&line)?;
                ensure!(
                    value.get("id").is_none(),
                    "Unexpected fixture server request"
                );
                self.fixture_notification(&value);
            }
            ensure!(
                self.fixture_terminals.remove(turn).as_deref() == Some("completed"),
                "Synthetic fixture turn failed"
            );
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(())
    }
    async fn exchange(&mut self, method: &str, params: Value) -> Result<Value> {
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
                            return Err(ReadFailure::Unsupported(
                                UnsupportedReason::UnsupportedMethod,
                            )
                            .into());
                        }
                        bail!("Codex RPC error: {}", e.get("code").unwrap_or(&Value::Null));
                    }
                    return value
                        .get("result")
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("Missing RPC result"));
                }
                self.fixture_notification(&value);
                ensure!(
                    value.get("id").is_none(),
                    "Unexpected server request or reply"
                );
                ensure!(
                    self.fixture
                        || value.get("method").and_then(Value::as_str) != Some("thread/started"),
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
                    .ok_or(ReadFailure::Unsupported(UnsupportedReason::PageData))?
                    .iter()
                    .cloned(),
            );
            match page.get("nextCursor") {
                Some(Value::Null) => return Ok(data),
                Some(Value::String(cursor)) => {
                    ensure!(cursors.insert(cursor.clone()), "Repeated page cursor");
                    params["cursor"] = json!(cursor);
                }
                _ => return Err(ReadFailure::Unsupported(UnsupportedReason::PageCursor).into()),
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
        self.group.terminate();
        let _ = self.child.kill().await;
        self.child.wait().await?;
        Ok(())
    }
}
#[derive(Debug)]
pub enum Capture {
    Ready(Box<Evidence>),
    Pending,
    Missing,
    Unsupported(UnsupportedReason),
    OverLimit,
    Excluded,
    WrongProject,
}

pub const MIN_READER_VERSION: &str = "0.162.0-alpha.17.2";
pub const MIN_HISTORY_VERSION: &str = "0.162.0-alpha.2";
fn admitted(version: &str, minimum: &str) -> bool {
    version.len() <= 128
        && semver::Version::parse(version).is_ok_and(|v| {
            v.cmp_precedence(&semver::Version::parse(minimum).expect("constant version"))
                != std::cmp::Ordering::Less
        })
}
pub fn supported(version: &str) -> bool {
    admitted(version, MIN_READER_VERSION)
}
pub fn history_supported(version: &str) -> bool {
    admitted(version, MIN_HISTORY_VERSION)
}
// Capture compatibility does not certify generation's deny-hook/isolation contract.
pub fn generation_supported(version: &str) -> bool {
    matches!(version, "0.162.1" | "0.162.0-alpha.17.2")
}
/// Read only turn_context identity/cwd from the bounded local source artifact.
/// An absent context is never attributed from a thread's current cwd.
pub fn turn_cwd(home: &Path, metadata: &Value, turn: &str) -> Result<Option<PathBuf>> {
    let path = metadata
        .get("path")
        .and_then(Value::as_str)
        .ok_or(ReadFailure::Unsupported(UnsupportedReason::SourcePath))?;
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
                    .ok_or(ReadFailure::Unsupported(UnsupportedReason::TurnCwd))?,
            )
            .canonicalize()?;
            if cwd.as_ref().is_some_and(|p| *p != next) {
                return Err(
                    ReadFailure::Unsupported(UnsupportedReason::ConflictingTurnContext).into(),
                );
            }
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
    if !history_supported(version) {
        return Capture::Unsupported(UnsupportedReason::HistoryVersion);
    }
    if turn["id"] != key.turn {
        return Capture::Unsupported(UnsupportedReason::TurnIdentity);
    }
    let outcome = match turn["status"].as_str() {
        Some("completed") => "completed",
        Some("failed") => "failed",
        Some("interrupted") => "interrupted",
        Some("inProgress") => return Capture::Pending,
        _ => return Capture::Unsupported(UnsupportedReason::TurnStatus),
    };
    let Some(completed) = turn["completedAt"].as_i64() else {
        return Capture::Pending;
    };
    let Some(started) = turn["startedAt"].as_i64() else {
        return Capture::Unsupported(UnsupportedReason::StartedAt);
    };
    if completed < started {
        return Capture::Unsupported(UnsupportedReason::Timestamps);
    }
    let mut ids = HashSet::new();
    if items.iter().any(|i| {
        i["id"].as_str().is_none_or(|id| !ids.insert(id.to_owned())) || i["type"].as_str().is_none()
    }) {
        return Capture::Unsupported(UnsupportedReason::ItemIdentity);
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
        capture_runtime: None,
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
    let version = match history_version(thread) {
        Ok(version) => version,
        Err(reason) => return Ok(Capture::Unsupported(reason)),
    };
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
            Capture::Unsupported(UnsupportedReason::TurnContext)
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
        if entry["turnId"] != key.turn {
            return Ok(Capture::Unsupported(UnsupportedReason::ItemTurn));
        }
        items.push(
            entry
                .get("item")
                .cloned()
                .ok_or(ReadFailure::Unsupported(UnsupportedReason::ItemProjection))?,
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

#[cfg(test)]
mod cold_initialization_tests {
    use super::*;
    #[tokio::test]
    async fn isolated_reader_rejects_thread_started_during_initialize() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir()?;
        let binary = d.path().join("server.py");
        std::fs::write(
            &binary,
            r#"#!/usr/bin/env python3
import sys,json
if '--version' in sys.argv:
    print('codex-cli 0.162.1')
    sys.exit(0)
for line in sys.stdin:
    request=json.loads(line)
    if request.get('method')=='initialize':
        print(json.dumps({'method':'thread/started','params':{}}), flush=True)
        print(json.dumps({'id':request['id'],'result':{}}), flush=True)
"#,
        )?;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))?;
        assert!(Client::open(&binary, d.path(), d.path()).await.is_err());
        assert!(
            Client::open_isolated_reader(&binary, d.path(), d.path())
                .await
                .is_err()
        );
        let producer = Client::open_fixture(&binary, d.path(), d.path()).await?;
        producer.close().await?;
        Ok(())
    }
}
