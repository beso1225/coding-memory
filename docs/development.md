# Development Setup

The development environment supports Apple Silicon macOS (`aarch64-darwin`)
and x86-64 Linux (`x86_64-linux`). Distribution archives still target Apple
Silicon macOS only. The project uses one Cargo package with library and binary
entry points. The initial CLI provides
durable ingestion, evidence capture, and scoped CLI/MCP retrieval; see
[ingestion.md](ingestion.md) and [retrieval.md](retrieval.md) for operational
boundaries. Memory generation and publication are subsequent stages.

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
error handling, YAML frontmatter parsing, and the official MCP SDK, with tempfile
for tests. `publish = false` prevents
accidental crate publication; binary releases use GitHub Releases instead.
Original project material is licensed under MIT OR Apache-2.0. Distribution
checks and third-party notice handling are described in [release operations](releasing.md).

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
requests, and manual dispatch. Its reusable `Development checks` workflow runs
on `ubuntu-24.04`: it evaluates the flake with `nix flake check --no-build` and
runs `nix develop --command pkf run check`. The pinned toolchain, Nix lock file,
and check commands are shared with local development.

Native checks are selected from the full Git change range by
[`scripts/ci_scope.py`](../scripts/ci_scope.py). PR comparisons use the merge
base; deleted and renamed inputs are included. A failed comparison fails the
selection job rather than silently skipping validation. Normal consolidation,
retrieval, and documentation changes run Linux checks only. Changes to platform
integration (capture, runtime probes, generation, service/worker startup), their
tests, dependencies, build inputs, or CI configuration also run the macOS
regression suite and latest official Codex CLI capture fixture. Keep the native
path list current when adding platform integration modules.

The [runtime compatibility workflow](../.github/workflows/runtime-compatibility.yml)
also runs the full native checks and credential-free CLI fixture every Monday
at 03:17 UTC, and on manual dispatch. One Mac job shares the build between
regression tests and the runtime fixture. Local Worker runtime verification
continues independently. A manual CI dispatch requests all native checks and
an archive dry run. See [release operations](releasing.md) for selective archive
builds and publication.

The shared setup action caches the Nix store by OS, architecture, flake inputs,
and toolchain. Cargo development dependencies and `target/debug` are separately
cached by those inputs plus Cargo.lock; native rustup distribution caches use a
different namespace. Cached builds never bypass tests, release notice generation,
or archive smoke checks. Credentials, state, personal histories, and Vaults are
not cache inputs. No cache storage limit or paid spending limit is increased.
Cache restore/save overhead should be compared with saved build time after the
first successful runs; the initial run has no warm cache.

PR updates cancel superseded runs. Main runs are not canceled because a called
release may already be publishing. Ordinary main pushes run the Linux checks
once; version-changing pushes reuse that result before publication. The release
workflow runs its own Linux checks only when dispatched directly. All external
actions are pinned by commit SHA. Only the release call permits `contents: write`,
and only its guarded publish job requests a write token; check jobs use read-only
tokens. CI uses no LLM credentials and operates on isolated synthetic fixtures.

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
integration gates from #1 and #2 are documented separately. Scoped retrieval is
implemented for #6; login operation, extraction, and vault publication remain
subsequent issues.
