use anyhow::Result;
use coding_memory::{domain::RunKey, state::State};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};
use tempfile::tempdir;

struct Server(Child, String);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl Server {
    fn start(root: &Path) -> Result<Self> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_coding-memory"))
            .args(["--state-dir"])
            .arg(root)
            .args(["dashboard", "--serve", "--port", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = tx.send(line);
        });
        let mut server = Self(child, String::new());
        let line = rx.recv_timeout(Duration::from_secs(10))?;
        server.1 = line
            .trim()
            .strip_prefix("Dashboard: http://")
            .expect("server URL")
            .to_owned();
        assert!(server.1.starts_with("127.0.0.1:"));
        Ok(server)
    }
    fn request(&self, method: &str, path: &str, host: &str, extra: &str) -> Result<String> {
        let mut stream = TcpStream::connect(&self.1)?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: {host}\r\n{extra}\r\n"
        )?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        Ok(response)
    }
    fn get(&self, path: &str) -> Result<String> {
        self.request("GET", path, &self.1, "")
    }
}

#[test]
fn live_snapshots_refresh_existing_state_without_claims_or_content() -> Result<()> {
    let dir = tempdir()?;
    let mut state = State::open(&dir.path().join("state.sqlite"))?;
    state.register_project("example<script>", "/synthetic", 1)?;
    let server = Server::start(dir.path())?;
    let page = server.get("/")?;
    assert!(page.starts_with("HTTP/1.1 200"));
    assert!(page.contains("/dashboard.js"));
    assert!(page.contains("<main><p id=\"refresh-status\""));
    assert!(page.contains("script-src 'self'"));
    assert!(page.contains("connect-src 'self'"));
    let first = server.get("/snapshot")?;
    assert!(first.contains("0 records (data available)"));
    assert!(first.contains("Cache-Control: no-store"));
    state.enqueue(
        &RunKey {
            store: "s".into(),
            thread: "private thread".into(),
            turn: "t".into(),
        },
        "example<script>",
        "/synthetic",
        1,
    )?;
    let second = server.get("/snapshot")?;
    assert!(second.contains("example&lt;script&gt;"));
    assert!(second.contains("queued"));
    assert!(!second.contains("private thread"));
    assert!(!second.contains("Regenerate"));
    assert_eq!(state.status()?[0].state, "queued");
    assert!(!dir.path().join("worker.lock").exists());
    let script = server.get("/dashboard.js")?;
    assert!(script.starts_with("HTTP/1.1 200"));
    assert!(script.contains("Content-Type: text/javascript"));
    Ok(())
}

#[test]
fn live_server_rejects_other_origins_hosts_methods_and_paths() -> Result<()> {
    let dir = tempdir()?;
    let missing = dir.path().join("missing");
    let server = Server::start(&missing)?;
    for (method, path, host, extra, code) in [
        ("POST", "/snapshot", server.1.as_str(), "", "405"),
        ("GET", "/snapshot", "example.com", "", "403"),
        (
            "GET",
            "/snapshot",
            server.1.as_str(),
            "Origin: https://example.com\r\n",
            "403",
        ),
        (
            "GET",
            "/snapshot",
            server.1.as_str(),
            "Sec-Fetch-Site: cross-site\r\n",
            "403",
        ),
        ("GET", "/../state.sqlite", server.1.as_str(), "", "404"),
    ] {
        let response = server.request(method, path, host, extra)?;
        assert!(
            response.starts_with(&format!("HTTP/1.1 {code}")),
            "{response}"
        );
    }
    let snapshot = server.get("/snapshot")?;
    assert!(snapshot.contains("data-unavailable"));
    assert!(!missing.exists());
    Ok(())
}
