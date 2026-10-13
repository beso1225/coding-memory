# coding-memory

Long-term memory for coding agents, built around a Rust engine and a Markdown
vault. The engine captures completed runs, extracts reusable knowledge, reviews
consolidation proposals, and exposes stored memories through MCP.

The development environment and a single Rust package scaffold are available.
The engine, CLI behavior, and runtime adapters have not been implemented yet.

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
