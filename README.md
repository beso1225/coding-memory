# coding-memory

Long-term memory for coding agents, built around a Rust engine and a Markdown
vault. The engine captures completed runs, extracts reusable knowledge, reviews
consolidation proposals, and exposes stored memories through MCP.

The project is currently in the design phase. No engine or runtime adapter has
been implemented yet.

## Documentation

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
