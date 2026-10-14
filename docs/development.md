# Development Setup

The initial environment targets Apple Silicon macOS (`aarch64-darwin`). It uses
one Cargo package with library and binary entry points. The initial CLI provides
durable ingestion and evidence capture; see [ingestion.md](ingestion.md) for
explicit project registration and operational boundaries. Memory generation and
retrieval are subsequent stages.

## Prerequisites

- Nix with the `nix-command` and `flakes` experimental features enabled.
- direnv with its shell hook enabled if automatic activation is desired.

No permanent tool installation, global shell configuration change, worker
registration, model download, or provider credential is required by this setup.
Codex and ChatGPT App remain separately installed runtimes for later integration
experiments; this flake does not replace or pin their installations.

## Activate the environment

From the repository root, review `.envrc` and allow it:

```sh
direnv allow
pkf run check
```

`.envrc` contains only `use flake`. It loads the declared development shell;
it does not install Git hooks or start a worker.

For a shell without direnv, or a one-shot command:

```sh
nix develop
nix develop --command pkf run check
```

The first activation can download Nix inputs and tool binaries. `flake.lock`
fixes input revisions, `rust-toolchain.toml` selects Rust `1.99.0`, and the pkfire
input and Pkl task schema both select `0.16.0`. Commit lock files with intentional
dependency changes. Nix's Git-backed flake source includes tracked files; a newly
added file imported by the flake must be added to Git, or registered with
`git add -N`, before it can be evaluated.

These stable versions were verified on 2026-10-13 against the
[Rust release announcement](https://blog.rust-lang.org/2026/10/01/Rust-1.99.0/)
and the [pkfire release](https://github.com/mizchi/pkfire/releases/tag/pkfire%400.16.0).
The pkfire `v0.16.0` tag includes the updated Nix binary metadata; the original
`pkfire@0.16.0` package tag still contains older binary metadata. Use the `v` tag
for the flake and the package URI above for the Pkl schema.

## Included tools

- Rust compiler and Cargo, with Clippy, rustfmt, and standard-library source.
- rust-analyzer for editor integration.
- SQLite CLI and pkg-config for storage development.
- pkfire and Pkl for task definitions.
- nixfmt for the environment definition.
- actionlint for GitHub Actions workflow validation.
- Python for standard-library-only capture experiments and offline fixture tests.

The Rust selection comes from `rust-toolchain.toml`, using rust-overlay in Nix.
It is also understood by rustup users. The editor should inherit the development
shell so it finds the selected toolchain and `RUST_SRC_PATH`.

Cargo.lock pins SQLite (bundled through rusqlite), Serde, Tokio, Clap, SHA-256,
and error handling, with tempfile for tests. `publish = false` prevents
accidental crate publication while the project and its license are being defined.

## Tasks

| Command | Behavior |
| --- | --- |
| `pkf list` | List development tasks |
| `pkf run fmt` | Format Rust, `flake.nix`, and `Taskfile.pkl` in place |
| `pkf run fmt-check` | Check all three formats without modifying files |
| `pkf run lint` | Run Clippy on all targets/features with warnings denied |
| `pkf run test` | Run Rust tests and documentation tests |
| `pkf run build` | Build library and binary targets |
| `pkf run workflow-lint` | Validate GitHub Actions workflows with actionlint |
| `pkf run capture-fixtures` | Check recorded Codex capture contracts without runtime/model calls |
| `pkf run check` | Run format, lint, test, build, workflow, and capture fixture checks |
| `pkf run` | Run the same aggregate checks by default |

Tasks use Cargo's lock file and incremental compilation. pkfire result caching
is disabled for these wrappers so a previous successful check cannot hide a
toolchain/environment change or a missing build artifact.

Rust behavior tests exercise durable queue recovery, worktree identity, evidence
integrity, and a synthetic stdio reader. Add focused regression tests alongside
behavior changes. Deterministic tests do not establish LLM extraction quality.

The separate capture fixture tests validate the observed runtime contracts in
[the capture experiment](../experiments/codex_capture/README.md). Live experiments
are opt-in and are never launched by `pkf run check` or CI.

The same Rust checks can be invoked directly inside the environment:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo build --locked --all-targets
```

## Continuous integration

[The CI workflow](../.github/workflows/ci.yml) runs on pushes to `main`, pull
requests, and manual dispatch. It uses the ARM64 `macos-15` runner to match the
development flake's `aarch64-darwin` target, evaluates the flake with
`nix flake check --no-build`, and runs `nix develop --command pkf run check`.
The workflow uses the committed toolchain and Nix lock file. Action versions
are pinned by commit SHA, and the token has only `contents: read` permission.

CI requires no LLM credentials and does not launch a worker or modify the memory
vault. Additional platforms and build caching can be added when the engine's
implementation needs them.

## Updating tools and keeping state local

Change Rust deliberately in `rust-toolchain.toml`. For pkfire, keep the flake
input and `Taskfile.pkl` package URI on the same version. Refresh `flake.lock`
deliberately and rerun `pkf run check` after a tool update.

`target/` and `.direnv/` are ignored. Captured evidence, runtime SQLite state,
and provider credentials do not belong in either the source repository or the
development shell definition. The separate memory vault is not modified by
development setup.

The setup and ingestion implementation support
[issue #3](https://github.com/beso1225/coding-memory/issues/3). Capture and generation
integration gates from #1 and #2 are documented separately; login operation,
extraction, vault publication, and retrieval remain subsequent issues.
