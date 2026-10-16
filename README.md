# coding-memory

Long-term memory for coding agents, built around a Rust engine and a Markdown
vault. The engine captures completed runs, extracts reusable knowledge, reviews
consolidation proposals, and exposes stored memories through MCP.

The Rust CLI implements project registration, durable run ingestion, bounded
Codex history capture, immutable local evidence, scoped FTS retrieval, and read-only
MCP tools, evidence-grounded candidate extraction, reviewed consolidation, and
recoverable Vault updates with independently authenticated Codex execution.
A bounded background worker and explicit macOS LaunchAgent configuration connect
the pipeline. An initial synthetic real-model evaluation demonstrated separate-session
MCP retrieval and knowledge application; broader quality assessment remains ongoing.

## Install

Release archives target Apple Silicon Macs running macOS 15 or later.
See [installation, updates, and recovery](docs/install.md). The first release
candidate is published only after the release workflow succeeds; Homebrew
distribution follows RC installation validation. Codex is installed separately.

## Development

With Nix and direnv installed (Apple Silicon macOS or x86-64 Linux):

```sh
direnv allow
pkf run check
```

Without automatic direnv activation:

```sh
nix develop --command pkf run check
```

See [Development setup](docs/development.md) for pinned tools, tasks, and the
scope of the scaffold.

## Documentation

- [Development setup](docs/development.md): environment activation and checks.
- [Release operations](docs/releasing.md): release PRs, validation, publication,
  and retries.
- [License review](docs/license-review.md): dependency and reference-project
  licensing, attribution, and packaging requirements.
- [Durable ingestion](docs/ingestion.md): collection commands, state recovery,
  source boundaries, and isolated runtime validation.
- [Local App capture diagnostics](docs/app-capture.md): creation-version boundaries,
  metadata-only diagnosis and specific held reasons.
- [Scoped retrieval](docs/retrieval.md): Markdown schema, index rebuilding,
  scoped search/read, MCP setup, and runtime validation.
- [Evidence-grounded extraction](docs/extraction.md): candidate staging, external
  transmission policy, independent login, retry handling, and execution limits.
- [Reviewed consolidation](docs/consolidation.md): triggers, independent review,
  Wikilinks, durable Vault application/recovery, and opt-in evidence cleanup.
- [Background worker](docs/worker.md): explicit configuration, macOS startup,
  singleton ownership, status and recovery.
- [Initial quality evaluation](docs/quality-evaluation.md): real-model synthetic
  corpus, separate-session reuse and observed limitations.
- [Design](docs/design.md): agreed scope, data model, component contracts, and
  persistence behavior.
- [Runtime and OSS research](docs/runtime-research.md): inspected revisions,
  evidence, limitations, and required integration experiments.
- [Roadmap](docs/roadmap.md): registered milestones, validation gates, and the
  initial GitHub issues.
- [Glossary](GLOSSARY.md): domain terminology.

The separate `coding-memory-vault` repository holds the authoritative memory
notes. Runtime state and captured evidence stay in a local state directory
outside both repositories.

## License

Original Coding Memory code and original repository documentation are licensed
under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
Third-party components retain their own licenses; release archives include
dependency notices and Rust library attributions in `release-notices/`.
This software license does not license users' Vaults, captured conversations,
evidence, or other user data.
