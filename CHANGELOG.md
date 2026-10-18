# Changelog

## [0.1.0-rc.4]

Bounded, resumable extraction and a live read-only status dashboard for Apple
Silicon macOS 15+. This release preserves state schema version 1, existing Worker
configuration, Markdown notes, immutable evidence and independent generation login.

- Read immutable evidence locally up to 16 MiB and partition its items into
  contiguous chunks that each fit the 256 KiB request budget, so a completed run
  whose evidence exceeds the old single-request limit is extracted instead of held.
- Commit candidates only after every chunk succeeds, and resume an interrupted or
  rejected chunked extraction at the first failed chunk instead of repeating calls
  that already succeeded. A changed evidence digest, prompt version, generator
  (another model, backend, provider or Engine version) or partition restarts every
  chunk; a fence event clears the record, and the record never publishes candidates
  by itself.
- Define the model input budget once as the complete prompt plus the serialized
  response schema, and enforce that same sum in extraction, consolidation and the
  generation backend. The undocumented extra slack is removed.
- Split evidence holds into absent, unreadable and invalid reasons, keep source,
  framing and single-item overflow distinct, and complete a captured bundle with no
  native items as `succeeded_empty` without a model call.
- Record provenance and a bounded reason for every failure inside the extraction
  loop, including one while preparing a later chunk, instead of leaving a claimed
  job `running`; only an unwritable state database falls back to lease recovery.
- Add `dashboard` for a standalone HTML snapshot and `dashboard --serve` for a live
  viewer bound to `127.0.0.1`, showing worker lock ownership, cycle attempts and
  failures, project-scoped stage/state counts, attention reasons with recovery
  guidance, successful empty extraction and recent applied notes.
- Cover the multi-call extraction path end to end in the Codex loopback probe,
  including a rejected citation to an item held in another chunk.
- Keep the macOS compatibility suite out of ordinary pull requests and scope the
  remaining checks by changed paths; it runs in its own workflow.

Capture admission remains reader >= `0.162.0-alpha.17.2` and history creation
>= `0.162.0-alpha.2`, with format validation and periodic reader verification.
Generation isolation remains gated to `0.162.1` or `0.162.0-alpha.17.2`; this release
does not expand that generation allowlist. A changed Engine fixture queues reader
revalidation. State schema version 1 is unchanged: the new per-job chunk record is
created on open like every other stage table, and existing state, notes and
configuration need no migration.

Stop the Worker and back up state/configuration and the Vault before replacing the
binary. Jobs an earlier release held as `held_invalid_or_oversized_evidence` are
preserved and need a deliberate `extraction-retry`; bounded extraction then
processes valid evidence in chunks and resumes a retry at the first failed chunk.
Sources above the local 16 MiB ceiling, framing that cannot fit or a single
oversized item stay not retryable until the source is captured smaller, and a
missing or unreadable source becomes retryable only after it is restored. Existing
held work is never released automatically. Restart the Worker and MCP clients after
upgrading; the dashboard viewer is read-only and can be stopped without affecting
the Worker.

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
