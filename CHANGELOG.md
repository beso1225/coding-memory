# Changelog

## [0.1.0-rc.2]

First installable private release candidate for Apple Silicon macOS 15+.
It includes the capture, extraction, consolidation, MCP retrieval, and bounded
Worker features described below. Codex remains separately installed.

- Find unpublished drafts through the paginated release list and verify their
  uploaded assets by release ID. The by-tag endpoint excludes drafts.
- Resume a same-commit draft without creating a duplicate release.
- Report GitHub CLI error details when publishing fails.

The rc.1 attempt passed build and archive tests but stopped before publication
because its draft lookup failed. Its tag is retained as an attempt record.
This change does not alter runtime behavior or the state/configuration schema.

## [0.1.0-rc.1]

Initial release candidate for Apple Silicon Macs running macOS 15 or later.

- Capture completed coding-agent runs into durable local evidence.
- Extract and review reusable knowledge before applying Markdown Vault changes.
- Retrieve scoped memories through a read-only MCP server.
- Run a bounded background worker with explicit LaunchAgent configuration.
- Distribute a CLI archive with checksums, original project licenses, and
  third-party notices. Codex remains a separately installed runtime.

This release candidate establishes the distribution pipeline. It does not
establish compatibility with all future Codex versions or production readiness.
