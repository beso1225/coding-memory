//! Harness-specific MCP client configuration for the shared read-only server.
//!
//! The MCP server (`crate::mcp`) is harness-agnostic: it speaks MCP over stdio
//! and is bound to one registered project by absolute CLI arguments. Each
//! harness declares that same command in its own format, so this module renders
//! one server entry per supported harness from identical inputs. It writes no
//! configuration and never modifies a harness installation.
use crate::notes;
use anyhow::{Context, Result, ensure};
use std::{fmt::Write as _, path::Path};

/// Tightest server-name bound among the supported harnesses (DeepSeek Harness).
const SERVER_NAME_LIMIT: usize = 32;
/// DeepSeek Harness MCP client plugin package that bridges stdio servers.
const DSH_PLUGIN: &str = "@deepseek-ai/dsh-mcp-client";

/// A supported MCP client harness and its configuration format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Harness {
    /// Codex CLI and App: `[mcp_servers.<name>]` in `config.toml`.
    Codex,
    /// OpenCode V2: `mcp.servers.<name>` in `opencode.json(c)`.
    Opencode,
    /// DeepSeek Harness: a `dsh-mcp-client` Cordis overlay.
    Dsh,
}

/// Render the `mcp` server entry for `harness` with resolved absolute paths.
///
/// The state directory and Vault must already exist; both are canonicalized so
/// the entry does not depend on a symlinked or relative location. The binary is
/// validated and recorded exactly as given, the way `service-plist` records it:
/// the documented install exposes a stable symlink whose target is a versioned
/// directory, so canonicalizing it would pin the entry to a single release.
pub fn mcp_config(
    harness: Harness,
    name: &str,
    binary: &Path,
    root: &Path,
    vault: &Path,
    project_id: &str,
) -> Result<String> {
    server_name(name)?;
    notes::identity(project_id)?;
    crate::worker::executable(binary)?;
    for (label, path) in [("State directory", root), ("Vault", vault)] {
        ensure!(
            path.is_absolute() && path.is_dir(),
            "{label} must be an absolute directory"
        );
    }
    let binary = text(binary)?;
    let root = text(&root.canonicalize()?)?;
    let vault = text(&vault.canonicalize()?)?;
    let args = arguments(&root, &vault, project_id);
    match harness {
        Harness::Codex => Ok(codex(name, &binary, &args)),
        Harness::Opencode => {
            opencode(name, &binary, &args).context("Failed to render the OpenCode configuration")
        }
        Harness::Dsh => {
            dsh(name, &binary, &args).context("Failed to render the DeepSeek Harness overlay")
        }
    }
}
/// The shared `--state-dir ... mcp --vault ... --project-id ...` argument tail.
fn arguments(root: &str, vault: &str, project_id: &str) -> Vec<String> {
    [
        "--state-dir",
        root,
        "mcp",
        "--vault",
        vault,
        "--project-id",
        project_id,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
/// Codex `config.toml` table with a single-line argument array.
fn codex(name: &str, binary: &str, args: &[String]) -> String {
    let list = args
        .iter()
        .map(|value| toml_string(value))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "[mcp_servers.{name}]\ncommand = {}\nargs = [{list}]\n",
        toml_string(binary)
    )
}
/// OpenCode V2 `opencode.json(c)` document. The caller merges the `mcp.servers`
/// entry into any existing configuration.
#[derive(serde::Serialize)]
struct OpencodeDocument<'a> {
    #[serde(rename = "$schema")]
    schema: &'a str,
    mcp: OpencodeMcp<'a>,
}
#[derive(serde::Serialize)]
struct OpencodeMcp<'a> {
    servers: std::collections::BTreeMap<&'a str, OpencodeServer>,
}
#[derive(serde::Serialize)]
struct OpencodeServer {
    #[serde(rename = "type")]
    kind: &'static str,
    command: Vec<String>,
}
fn opencode(name: &str, binary: &str, args: &[String]) -> Result<String> {
    let mut command = vec![binary.to_owned()];
    command.extend(args.iter().cloned());
    let mut servers = std::collections::BTreeMap::new();
    servers.insert(
        name,
        OpencodeServer {
            kind: "local",
            command,
        },
    );
    let document = OpencodeDocument {
        schema: "https://opencode.ai/config.json",
        mcp: OpencodeMcp { servers },
    };
    Ok(format!("{}\n", serde_json::to_string_pretty(&document)?))
}
/// DeepSeek Harness Cordis overlay that inserts the `dsh-mcp-client` plugin.
#[derive(serde::Serialize)]
struct DshPatch<'a> {
    insert: Vec<DshEntry<'a>>,
}
#[derive(serde::Serialize)]
struct DshEntry<'a> {
    id: String,
    name: &'a str,
    config: DshConfig<'a>,
}
#[derive(serde::Serialize)]
struct DshConfig<'a> {
    #[serde(rename = "serverName")]
    server_name: &'a str,
    transport: &'a str,
    command: &'a str,
    args: &'a [String],
}
fn dsh(name: &str, binary: &str, args: &[String]) -> Result<String> {
    let document = vec![DshPatch {
        insert: vec![DshEntry {
            id: format!("mcp-{}", name.replace('_', "-")),
            name: DSH_PLUGIN,
            config: DshConfig {
                server_name: name,
                transport: "stdio",
                command: binary,
                args,
            },
        }],
    }];
    Ok(serde_yaml_ng::to_string(&document)?)
}
/// A server name that stays a bare key or identifier in every supported format.
fn server_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= SERVER_NAME_LIMIT
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "Invalid MCP server name"
    );
    Ok(())
}
fn text(path: &Path) -> Result<String> {
    let value = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Non-UTF8 configuration path"))?;
    ensure!(
        !value.chars().any(char::is_control),
        "Control character in configuration path"
    );
    Ok(value.to_owned())
}
/// A TOML basic string. Resolved paths are UTF-8 and control-free, so only
/// quotes, backslashes, and any remaining control characters need escaping.
fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_strings_escape_quotes_backslashes_and_controls() {
        assert_eq!(toml_string(r#"a"b\c"#), "\"a\\\"b\\\\c\"");
        assert_eq!(toml_string("a\nb\tc"), "\"a\\nb\\tc\"");
        assert_eq!(toml_string("a\u{7f}b"), "\"a\\u007Fb\"");
    }
    #[test]
    fn server_names_are_bounded_and_bare() {
        assert!(server_name("coding_memory").is_ok());
        assert!(server_name("a-b_c1").is_ok());
        assert!(server_name(&"a".repeat(SERVER_NAME_LIMIT)).is_ok());
        assert!(server_name("").is_err());
        assert!(server_name(&"a".repeat(SERVER_NAME_LIMIT + 1)).is_err());
        assert!(server_name("has space").is_err());
        assert!(server_name("has.dot").is_err());
        assert!(server_name("a\"b").is_err());
    }
    #[test]
    fn configuration_paths_must_be_utf8_and_control_free() {
        assert!(text(Path::new("/tmp/state")).is_ok());
        assert!(text(Path::new("/tmp/two\nlines")).is_err());
        assert!(text(Path::new("/tmp/tab\there")).is_err());
        #[cfg(unix)]
        {
            use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
            assert!(text(Path::new(OsStr::from_bytes(b"/tmp/\xff"))).is_err());
        }
    }
}
