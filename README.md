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

## Development

On an Apple Silicon Mac with Nix and direnv installed:

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
- [Durable ingestion](docs/ingestion.md): collection commands, state recovery,
  source boundaries, and isolated runtime validation.
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
