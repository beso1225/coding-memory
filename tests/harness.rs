//! Deterministic coverage for harness MCP configuration rendering.
use coding_memory::harness::{Harness, mcp_config};
use std::{
    fs,
    path::{Path, PathBuf},
};

struct Fixture {
    _root: tempfile::TempDir,
    state: PathBuf,
    vault: PathBuf,
    binary: PathBuf,
}

/// An absolute binary, state directory, and Vault. Directories are canonicalized
/// the way `mcp_config` resolves them.
fn fixture() -> anyhow::Result<Fixture> {
    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let vault = root.path().join("vault");
    fs::create_dir_all(&state)?;
    fs::create_dir_all(&vault)?;
    let binary = root.path().join("coding-memory");
    fs::write(&binary, b"#!/bin/sh\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
    }
    Ok(Fixture {
        _root: root,
        state: state.canonicalize()?,
        vault: vault.canonicalize()?,
        binary,
    })
}

fn argv(state: &Path, vault: &Path, project: &str) -> Vec<String> {
    vec![
        "--state-dir".into(),
        state.to_str().unwrap().into(),
        "mcp".into(),
        "--vault".into(),
        vault.to_str().unwrap().into(),
        "--project-id".into(),
        project.into(),
    ]
}

#[test]
fn codex_entry_is_a_paste_ready_table() -> anyhow::Result<()> {
    let f = fixture()?;
    let text = mcp_config(
        Harness::Codex,
        "coding_memory",
        &f.binary,
        &f.state,
        &f.vault,
        "synthetic-project",
    )?;
    assert_eq!(
        text,
        format!(
            "[mcp_servers.coding_memory]\ncommand = \"{}\"\nargs = [\"--state-dir\", \"{}\", \"mcp\", \"--vault\", \"{}\", \"--project-id\", \"synthetic-project\"]\n",
            f.binary.display(),
            f.state.display(),
            f.vault.display()
        )
    );
    Ok(())
}

#[test]
fn opencode_entry_is_one_local_stdio_server() -> anyhow::Result<()> {
    let f = fixture()?;
    let text = mcp_config(
        Harness::Opencode,
        "coding_memory",
        &f.binary,
        &f.state,
        &f.vault,
        "synthetic-project",
    )?;
    let value: serde_json::Value = serde_json::from_str(&text)?;
    assert_eq!(value["$schema"], "https://opencode.ai/config.json");
    let servers = value["mcp"]["servers"].as_object().unwrap();
    assert_eq!(servers.len(), 1);
    let server = &servers["coding_memory"];
    assert_eq!(server["type"], "local");
    let command = server["command"].as_array().unwrap();
    let mut expected = vec![f.binary.to_str().unwrap().to_owned()];
    expected.extend(argv(&f.state, &f.vault, "synthetic-project"));
    assert_eq!(
        command,
        &expected
            .iter()
            .map(|a| serde_json::Value::String(a.clone()))
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn dsh_entry_is_a_cordis_overlay() -> anyhow::Result<()> {
    let f = fixture()?;
    let text = mcp_config(
        Harness::Dsh,
        "coding_memory",
        &f.binary,
        &f.state,
        &f.vault,
        "synthetic-project",
    )?;
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text)?;
    let patch = value.as_sequence().unwrap();
    assert_eq!(patch.len(), 1);
    let entry = &patch[0]["insert"][0];
    assert_eq!(entry["id"].as_str().unwrap(), "mcp-coding-memory");
    assert_eq!(
        entry["name"].as_str().unwrap(),
        "@deepseek-ai/dsh-mcp-client"
    );
    let config = &entry["config"];
    assert_eq!(config["serverName"].as_str().unwrap(), "coding_memory");
    assert_eq!(config["transport"].as_str().unwrap(), "stdio");
    assert_eq!(
        config["command"].as_str().unwrap(),
        f.binary.to_str().unwrap()
    );
    let args: Vec<&str> = config["args"]
        .as_sequence()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(args, argv(&f.state, &f.vault, "synthetic-project"));
    Ok(())
}

#[test]
fn a_custom_server_name_is_used_by_every_harness() -> anyhow::Result<()> {
    let f = fixture()?;
    let codex = mcp_config(
        Harness::Codex,
        "team_memory",
        &f.binary,
        &f.state,
        &f.vault,
        "p",
    )?;
    assert!(codex.starts_with("[mcp_servers.team_memory]\n"));
    let opencode: serde_json::Value = serde_json::from_str(&mcp_config(
        Harness::Opencode,
        "team_memory",
        &f.binary,
        &f.state,
        &f.vault,
        "p",
    )?)?;
    assert!(opencode["mcp"]["servers"]["team_memory"].is_object());
    let dsh: serde_yaml_ng::Value = serde_yaml_ng::from_str(&mcp_config(
        Harness::Dsh,
        "team_memory",
        &f.binary,
        &f.state,
        &f.vault,
        "p",
    )?)?;
    assert_eq!(
        dsh[0]["insert"][0]["id"].as_str().unwrap(),
        "mcp-team-memory"
    );
    assert_eq!(
        dsh[0]["insert"][0]["config"]["serverName"]
            .as_str()
            .unwrap(),
        "team_memory"
    );
    Ok(())
}

#[test]
fn paths_with_spaces_round_trip() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let state = root
        .path()
        .join("Application Support")
        .join("coding-memory");
    fs::create_dir_all(&state)?;
    let binary = root.path().join("coding-memory");
    fs::write(&binary, b"#!/bin/sh\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
    }
    let state = state.canonicalize()?;
    let vault = state.clone();
    for harness in [Harness::Codex, Harness::Opencode, Harness::Dsh] {
        let text = mcp_config(harness, "coding_memory", &binary, &state, &vault, "p")?;
        assert!(text.contains(state.to_str().unwrap()));
    }
    Ok(())
}

#[test]
fn invalid_inputs_are_rejected() -> anyhow::Result<()> {
    let f = fixture()?;
    let cases: [(&str, PathBuf); 2] = [
        ("relative state", PathBuf::from("relative")),
        ("missing vault", f._root.path().join("missing")),
    ];
    for (label, path) in cases {
        assert!(
            mcp_config(
                Harness::Codex,
                "coding_memory",
                &f.binary,
                &f.state,
                &path,
                "p"
            )
            .is_err(),
            "{label} must be rejected"
        );
    }
    assert!(
        mcp_config(
            Harness::Codex,
            "coding_memory",
            std::path::Path::new("/nonexistent-coding-memory"),
            &f.state,
            &f.vault,
            "p"
        )
        .is_err()
    );
    for name in ["", "has space", "has.dot"] {
        assert!(
            mcp_config(Harness::Codex, name, &f.binary, &f.state, &f.vault, "p").is_err(),
            "server name {name:?} must be rejected"
        );
    }
    assert!(
        mcp_config(
            Harness::Codex,
            "coding_memory",
            &f.binary,
            &f.state,
            &f.vault,
            "has space"
        )
        .is_err()
    );
    Ok(())
}
