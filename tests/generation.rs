#![cfg(unix)]
use anyhow::Result;
use coding_memory::generation::{Backend, CodexExec, Failure, Request};
use serde_json::json;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    time::Duration,
};
use tempfile::tempdir;

fn fixture(root: &std::path::Path, body: &str) -> Result<CodexExec> {
    let binary = root.join("fake-codex");
    fs::write(
        &binary,
        format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'codex-cli 0.162.1'; exit 0; fi\nwhile [ $# -gt 0 ]; do if [ \"$1\" = -o ]; then shift; output=$1; fi; shift; done\ncat >/dev/null\n{body}\n"
        ),
    )?;
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))?;
    Ok(CodexExec {
        root: root.into(),
        binary,
        guard_binary: env!("CARGO_BIN_EXE_coding-memory").into(),
        model: "synthetic".into(),
        timeout: Duration::from_secs(3),
        loopback: Some("127.0.0.1:1".parse()?),
    })
}
fn request() -> Request {
    Request {
        attempt: "fixture-1".into(),
        prompt: "synthetic".into(),
        schema: json!({"type":"object"}),
    }
}
#[tokio::test]
async fn version_probe_is_bounded_on_both_streams() -> Result<()> {
    for stream in ["", " >&2"] {
        let d = tempdir()?;
        let backend = fixture(d.path(), "")?;
        fs::write(
            &backend.binary,
            format!("#!/bin/sh\ndd if=/dev/zero bs=65536 count=2{stream} 2>/dev/null\n"),
        )?;
        assert_eq!(
            backend.generate(request()).await.err(),
            Some(Failure::OutputLimit)
        );
    }
    Ok(())
}
#[tokio::test]
async fn tool_denials_override_process_failure_and_timeout() -> Result<()> {
    for ending in ["exit 1", "sleep 10"] {
        let d = tempdir()?;
        let marker = coding_memory::domain::hash(b"fixture");
        let body = format!(
            "echo '{{\"type\":\"thread.started\",\"thread_id\":\"fixture\"}}'\nprintf 'denied' > \"$CODEX_HOME/tool-attempts/{marker}\"\n{ending}"
        );
        let mut backend = fixture(d.path(), &body)?;
        backend.timeout = Duration::from_millis(500);
        assert_eq!(
            backend.generate(request()).await.err(),
            Some(Failure::ToolAttempt)
        );
    }
    // Native tool events must also win without a marker, including on timeout.
    for ending in ["exit 1", "sleep 10"] {
        let d = tempdir()?;
        let body = format!(
            "echo '{{\"type\":\"thread.started\",\"thread_id\":\"fixture\"}}'\necho '{{\"type\":\"item.started\",\"item\":{{\"type\":\"command_execution\"}}}}'\n{ending}"
        );
        let mut backend = fixture(d.path(), &body)?;
        backend.timeout = Duration::from_millis(500);
        assert_eq!(
            backend.generate(request()).await.err(),
            Some(Failure::ToolAttempt)
        );
    }
    Ok(())
}
#[tokio::test]
async fn exec_requires_native_identity_and_rejects_tools_and_malformed_streams() -> Result<()> {
    for (body, expected) in [
        (
            "echo '{}' > \"$output\"\necho '{\"type\":\"thread.started\",\"thread_id\":\"fixture\"}'",
            None,
        ),
        (
            "echo '{}' > \"$output\"\necho '{\"type\":\"thread.started\",\"thread_id\":\"fixture\"}'\necho '{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\"}}'",
            Some(Failure::ToolAttempt),
        ),
        (
            "echo '{}' > \"$output\"\necho 'bad JSON'",
            Some(Failure::InvalidOutput),
        ),
        ("echo '{}' > \"$output\"", Some(Failure::InvalidOutput)),
        ("exit 1", Some(Failure::ProcessFailure)),
        ("yes x", Some(Failure::OutputLimit)),
        ("sleep 10", Some(Failure::Timeout)),
    ] {
        let d = tempdir()?;
        let mut backend = fixture(d.path(), body)?;
        if expected == Some(Failure::Timeout) {
            backend.timeout = Duration::from_millis(100);
        }
        let result = backend.generate(request()).await;
        assert_eq!(result.as_ref().err().copied(), expected);
        if let Ok(result) = result {
            assert_eq!(result.thread_id.as_deref(), Some("fixture"));
            assert_eq!(result.bytes, b"{}\n");
        }
    }
    Ok(())
}
#[test]
fn prepare_does_not_follow_external_directory_symlinks() -> Result<()> {
    let root = tempdir()?;
    let outside = tempdir()?;
    fs::set_permissions(outside.path(), fs::Permissions::from_mode(0o755))?;
    symlink(outside.path(), root.path().join("generation"))?;
    assert!(fixture(root.path(), "")?.prepare().is_err());
    assert_eq!(
        fs::metadata(outside.path())?.permissions().mode() & 0o777,
        0o755
    );
    assert!(!outside.path().join("codex-home").exists());
    Ok(())
}
#[test]
fn hook_denies_even_invalid_input_and_records_no_tool_contents() -> Result<()> {
    let d = tempdir()?;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_coding-memory"))
        .args([
            "--state-dir",
            "/does/not/exist",
            "generation-deny-tool",
            "--marker-dir",
        ])
        .arg(d.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"session_id":"fixture","tool_input":{"command":"SECRET"}}"#)?;
    let out = child.wait_with_output()?;
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let marker = d.path().join(coding_memory::domain::hash(b"fixture"));
    assert_eq!(fs::read_to_string(marker)?, "tool request denied\n");
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_coding-memory"))
        .args([
            "--state-dir",
            "/does/not/exist",
            "generation-deny-tool",
            "--marker-dir",
        ])
        .arg(d.path())
        .stdin(std::process::Stdio::null())
        .output()?;
    assert_eq!(result.status.code(), Some(2));
    Ok(())
}
