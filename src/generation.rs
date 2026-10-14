//! Fresh Codex execution in an owned home, with bounded I/O and a deny policy.
use crate::{domain::hash, evidence, extraction, ingestion, state::State};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    future::Future,
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Timeout,
    ProcessFailure,
    OutputLimit,
    InvalidOutput,
    ToolAttempt,
    AuthUnavailable,
    UnsupportedRuntime,
    IsolationUnavailable,
}
impl Failure {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Timeout => "generation_timeout",
            Self::ProcessFailure => "generation_process_failure",
            Self::OutputLimit => "generation_output_limit",
            Self::InvalidOutput => "generation_invalid_output",
            Self::ToolAttempt => "generation_tool_attempt",
            Self::AuthUnavailable => "generation_auth_unavailable",
            Self::UnsupportedRuntime => "generation_unsupported_runtime",
            Self::IsolationUnavailable => "generation_isolation_unavailable",
        }
    }
    pub fn retryable(self) -> bool {
        matches!(self, Self::Timeout | Self::ProcessFailure)
    }
}
pub struct Request {
    pub attempt: String,
    pub prompt: String,
    pub schema: Value,
}
pub struct Output {
    pub bytes: Vec<u8>,
    pub thread_id: Option<String>,
    pub runtime_version: String,
}
pub trait Backend {
    fn provenance(&self) -> Value;
    fn external(&self) -> bool;
    fn generate(
        &self,
        request: Request,
    ) -> impl Future<Output = std::result::Result<Output, Failure>> + Send;
}
#[derive(Clone)]
pub struct CodexExec {
    pub root: PathBuf,
    pub binary: PathBuf,
    pub guard_binary: PathBuf,
    pub model: String,
    pub timeout: Duration,
    pub loopback: Option<SocketAddr>,
}
fn quote(path: &Path) -> Result<String> {
    let s = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Non-UTF8 generation path"))?;
    Ok(format!("'{}'", s.replace('\'', "'\\''")))
}
fn owned_directory(parent: &Path, name: &str) -> Result<PathBuf> {
    let path = parent.join(name);
    ensure!(
        !path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink()),
        "Refuse symlinked generation directory"
    );
    let dir = evidence::private_directory(&path)?;
    ensure!(
        dir.parent() == Some(parent),
        "Generation directory escapes parent"
    );
    Ok(dir)
}
impl CodexExec {
    pub fn home(&self) -> PathBuf {
        self.root.join("generation/codex-home")
    }
    pub fn prepare(&self) -> Result<PathBuf> {
        let root = self.root.canonicalize()?;
        let generation = owned_directory(&root, "generation")?;
        ensure!(
            generation.starts_with(&root),
            "Generation directory escapes engine state"
        );
        let home = owned_directory(&generation, "codex-home")?;
        ensure!(
            home.starts_with(&generation),
            "Generation home escapes engine state"
        );
        let markers = owned_directory(&home, "tool-attempts")?;
        ensure!(
            markers.starts_with(&home),
            "Tool markers escape generation home"
        );
        State::open(&root.join("state.sqlite"))?.own_store(&ingestion::store_id(&home)?)?;
        let hooks = home.join("hooks.json");
        ensure!(
            !hooks
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink()),
            "Refuse symlinked generation policy"
        );
        let command = format!(
            "{} --state-dir {} generation-deny-tool --marker-dir {}",
            quote(&self.guard_binary.canonicalize()?)?,
            quote(&root)?,
            quote(&markers)?
        );
        let bytes = serde_json::to_vec(
            &json!({"hooks":{"PreToolUse":[{"matcher":".*","hooks":[{"type":"command","command":command,"timeout":3}]}]}}),
        )?;
        let temp = home.join(format!(".hooks-{}.tmp", std::process::id()));
        let result = (|| -> Result<()> {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&temp, &hooks)?;
            std::fs::File::open(&home)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result?;
        Ok(home)
    }
    async fn execute(&self, request: Request) -> std::result::Result<Output, Failure> {
        if request.prompt.len() > extraction::MAX_INPUT + 4096
            || serde_json::to_vec(&request.schema).map_or(true, |s| s.len() > extraction::MAX_INPUT)
        {
            return Err(Failure::OutputLimit);
        }
        if self.model.trim().is_empty()
            || self.model.len() > 128
            || self.timeout.is_zero()
            || self.timeout > Duration::from_secs(600)
        {
            return Err(Failure::IsolationUnavailable);
        }
        let home = self.prepare().map_err(|_| Failure::IsolationUnavailable)?;
        if self.loopback.is_none() {
            let auth = home.join("auth.json");
            if !auth.is_file()
                || auth
                    .symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_symlink())
            {
                return Err(Failure::AuthUnavailable);
            }
        }
        let version = version(&self.binary, &home).await?;
        if !crate::codex::generation_supported(&version) {
            return Err(Failure::UnsupportedRuntime);
        }
        crate::notes::identity(&request.attempt).map_err(|_| Failure::IsolationUnavailable)?;
        let attempts = owned_directory(
            home.parent().ok_or(Failure::IsolationUnavailable)?,
            "attempts",
        )
        .map_err(|_| Failure::IsolationUnavailable)?;
        let work = attempts.join(&request.attempt);
        std::fs::create_dir(&work).map_err(|_| Failure::IsolationUnavailable)?;
        let schema = work.join("schema.json");
        let out = work.join("output.json");
        std::fs::write(
            &schema,
            serde_json::to_vec(&request.schema).map_err(|_| Failure::InvalidOutput)?,
        )
        .map_err(|_| Failure::ProcessFailure)?;
        let mut command = Command::new(&self.binary);
        command
            .args([
                "exec",
                "--skip-git-repo-check",
                "--ignore-user-config",
                "--ignore-rules",
                "--json",
                "--sandbox",
                "read-only",
                "--enable",
                "hooks",
                "--disable",
                "plugins",
                "--dangerously-bypass-hook-trust",
                "-c",
                "notify=[]",
                "-c",
                "project_doc_max_bytes=0",
                "-c",
                "web_search=\"disabled\"",
                "-c",
                "analytics.enabled=false",
                "-c",
                "cli_auth_credentials_store=\"file\"",
                "--model",
            ])
            .arg(&self.model)
            .arg("--output-schema")
            .arg(schema)
            .arg("-o")
            .arg(&out)
            .arg("-")
            .env("CODEX_HOME", &home)
            .env_remove("OPENAI_API_KEY")
            .env_remove("CODEX_API_KEY")
            .env_remove("OPENAI_BASE_URL")
            .current_dir(&work)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(address) = self.loopback {
            if !address.ip().is_loopback() {
                return Err(Failure::IsolationUnavailable);
            }
            command.args(["-c","model_provider=\"memory_loopback\"","-c"]).arg(format!("model_providers.memory_loopback={{name=\"Synthetic loopback\",base_url=\"http://{address}/v1\",wire_api=\"responses\",requires_openai_auth=false,request_max_retries=0,stream_max_retries=0}}"));
        }
        let process = run_command(
            command,
            request.prompt.into_bytes(),
            self.timeout,
            1024 * 1024,
            256 * 1024,
        )
        .await?;
        let mut thread = None;
        let mut tool = false;
        let mut malformed = false;
        for line in process
            .stdout
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
        {
            let event: Value = match serde_json::from_slice(line) {
                Ok(event) => event,
                Err(_) => {
                    malformed = true;
                    continue;
                }
            };
            if event["type"] == "thread.started" {
                thread = event["thread_id"].as_str().map(str::to_owned);
            }
            tool |= matches!(
                event["item"]["type"].as_str(),
                Some(
                    "command_execution"
                        | "file_change"
                        | "mcp_tool_call"
                        | "web_search"
                        | "collab_tool_call"
                )
            );
        }
        if tool
            || home.join("tool-attempts/unidentified").exists()
            || thread.as_ref().is_some_and(|id| {
                home.join("tool-attempts")
                    .join(hash(id.as_bytes()))
                    .exists()
            })
        {
            return Err(Failure::ToolAttempt);
        }
        if let Some(failure) = process.failure {
            return Err(failure);
        }
        if malformed {
            return Err(Failure::InvalidOutput);
        }
        if !thread
            .as_ref()
            .is_some_and(|id| !id.is_empty() && id.len() <= 256)
        {
            return Err(Failure::InvalidOutput);
        }
        if !out
            .symlink_metadata()
            .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
        {
            return Err(Failure::InvalidOutput);
        }
        let bytes = evidence::read_bounded(&out, extraction::MAX_OUTPUT).map_err(|e| {
            if e.downcast_ref::<std::io::Error>().is_some() {
                Failure::InvalidOutput
            } else {
                Failure::OutputLimit
            }
        })?;
        Ok(Output {
            bytes,
            thread_id: thread,
            runtime_version: version,
        })
    }
}
impl Backend for CodexExec {
    fn provenance(&self) -> Value {
        json!({"backend":"codex_exec","model":self.model,"role":"extraction","prompt_version":extraction::PROMPT_VERSION,"schema_version":1,"engine_version":env!("CARGO_PKG_VERSION"),"provider":if self.loopback.is_some(){"synthetic_loopback"}else{"openai"}})
    }
    fn external(&self) -> bool {
        true
    }
    fn generate(
        &self,
        request: Request,
    ) -> impl Future<Output = std::result::Result<Output, Failure>> + Send {
        self.execute(request)
    }
}
pub(crate) async fn version(binary: &Path, home: &Path) -> std::result::Result<String, Failure> {
    let mut probe = Command::new(binary);
    probe
        .arg("--version")
        .env("CODEX_HOME", home)
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .env_remove("OPENAI_BASE_URL");
    let result = run_command(probe, Vec::new(), Duration::from_secs(5), 4096, 16384).await?;
    if let Some(failure) = result.failure {
        return Err(failure);
    }
    Ok(std::str::from_utf8(&result.stdout)
        .map_err(|_| Failure::UnsupportedRuntime)?
        .trim()
        .strip_prefix("codex-cli ")
        .ok_or(Failure::UnsupportedRuntime)?
        .to_owned())
}
pub(crate) struct ProcessGroup {
    pid: u32,
    armed: bool,
}
impl ProcessGroup {
    pub(crate) fn new(pid: u32) -> Self {
        Self { pid, armed: true }
    }
    pub(crate) fn terminate(&mut self) {
        if self.armed {
            terminate_group(self.pid);
            self.armed = false;
        }
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if self.armed {
            terminate_group(self.pid);
        }
    }
}
struct ProcessOutput {
    stdout: Vec<u8>,
    failure: Option<Failure>,
}
/// Retain bounded partial stdout even on timeout/failure so denial evidence wins.
async fn run_command(
    mut command: Command,
    input: Vec<u8>,
    duration: Duration,
    stdout_limit: usize,
    stderr_limit: usize,
) -> std::result::Result<ProcessOutput, Failure> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|_| Failure::ProcessFailure)?;
    let pid = child.id().ok_or(Failure::ProcessFailure)?;
    let mut group = ProcessGroup { pid, armed: true };
    let mut stdin = child.stdin.take().ok_or(Failure::ProcessFailure)?;
    let stdout = child.stdout.take().ok_or(Failure::ProcessFailure)?;
    let stderr = child.stderr.take().ok_or(Failure::ProcessFailure)?;
    let mut out = Vec::new();
    let mut err = Vec::new();
    let run = async {
        let send = async move {
            stdin
                .write_all(&input)
                .await
                .map_err(|_| Failure::ProcessFailure)?;
            stdin
                .shutdown()
                .await
                .map_err(|_| Failure::ProcessFailure)?;
            drop(stdin);
            Ok(())
        };
        let (_, _, _, status) = tokio::try_join!(
            bounded(stdout, &mut out, stdout_limit),
            bounded(stderr, &mut err, stderr_limit),
            send,
            async { child.wait().await.map_err(|_| Failure::ProcessFailure) }
        )?;
        if !status.success() {
            return Err(Failure::ProcessFailure);
        }
        Ok(())
    };
    let failure = match timeout(duration, run).await {
        Ok(Ok(())) => None,
        Ok(Err(failure)) => Some(failure),
        Err(_) => Some(Failure::Timeout),
    };
    if failure.is_some() {
        terminate_group(pid);
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    group.armed = false;
    Ok(ProcessOutput {
        stdout: out,
        failure,
    })
}
async fn bounded(
    mut stream: impl AsyncRead + Unpin,
    bytes: &mut Vec<u8>,
    limit: usize,
) -> std::result::Result<(), Failure> {
    let mut chunk = [0; 8192];
    loop {
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|_| Failure::ProcessFailure)?;
        if n == 0 {
            return Ok(());
        }
        let remaining = limit - bytes.len();
        bytes.extend_from_slice(&chunk[..n.min(remaining)]);
        if n > remaining {
            return Err(Failure::OutputLimit);
        }
    }
}
fn terminate_group(pid: u32) {
    #[cfg(unix)]
    {
        if let Ok(pid) = i32::try_from(pid) {
            // This is the process group created for this invocation, never a shared runtime.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
}
/// Minimal deny hook: persist only session identity, never tool input or transcript text.
pub fn deny_tool(marker_dir: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() <= 65536, "Oversized policy hook input");
    let event: Value = serde_json::from_slice(bytes)?;
    let marker = event["session_id"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 256)
        .map(|id| hash(id.as_bytes()))
        .unwrap_or_else(|| "unidentified".into());
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(marker_dir.join(marker)) {
        Ok(mut file) => {
            file.write_all(b"tool request denied\n")?;
            file.sync_all()?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
