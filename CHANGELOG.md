# Changelog

## [0.1.0-rc.3]

Bounded consolidation context and read-only recovery diagnostics for Apple Silicon
macOS 15+. This release preserves state schema version 1, existing Worker
configuration, Markdown notes, immutable evidence and independent generation login.

- Supply only referenced evidence items to consolidation and independent review,
  retaining original bundle hashes and rejecting citations to omitted items.
- Select stable size-bounded partitions with a 160 KiB consolidation target,
  room for the maximum 64 KiB proposal, and the existing 256 KiB prompt hard cap.
  Defer candidates without consuming generation retries; hold only individually
  oversized required contexts and continue with later candidates.
- Expose bounded sizing diagnostics in `consolidation-status` and add
  `consolidation-preflight` for explicitly selected held/exhausted batches. Preview
  uses read-only state and a private temporary index without models or queue changes.
- Reuse the returned draft release ID after creation, avoiding stale release-list
  results before asset verification and publication.

Capture admission remains reader >= `0.162.0-alpha.17.2` and history creation
>= `0.162.0-alpha.2`, with format validation and periodic reader verification.
Generation isolation remains gated to `0.162.1` or `0.162.0-alpha.17.2`; this release
does not expand that generation allowlist. A changed Engine fixture queues reader
revalidation. Existing state/notes remain backward compatible; generated diagnostics
are additive and do not require migration.

Stop the Worker and back up state/configuration and the Vault before replacing the
binary. Run preflight on preserved consolidation holds before deliberately retrying
them, then restart/check the Worker and MCP clients. Preview is a current-state
estimate; normal processing repeats all validation. Existing held work is never
released automatically. Oversized extraction inputs remain a separate follow-up;
this release does not retry or discard them.

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
