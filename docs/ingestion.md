# Durable Ingestion

Issue: [#3](https://github.com/beso1225/coding-memory/issues/3).
Implemented on 2026-10-14 (Asia/Tokyo). This stage captures evidence; it does not
call a generation model, create memory notes, install hooks, or start a login worker.

## Run the collector

Build with `nix develop --command cargo build`. Use absolute executable and state
paths in a runtime hook. Choose a dedicated local state directory outside both Git
repositories. The commands below require explicit locations; replace the example
paths with your installation. Register only projects whose history you want captured.

```sh
coding-memory --state-dir /absolute/engine-state register \
  --codex-home /absolute/codex-state --codex-binary /absolute/codex \
  --project /absolute/project

coding-memory --state-dir /absolute/engine-state reconcile \
  --codex-home /absolute/codex-state --codex-binary /absolute/codex \
  --project /absolute/project

coding-memory --state-dir /absolute/engine-state capture \
  --codex-home /absolute/codex-state --codex-binary /absolute/codex \
  --project /absolute/project

coding-memory --state-dir /absolute/engine-state status
coding-memory --state-dir /absolute/engine-state evidence SHA256_DIGEST
```

Registration obtains a bounded baseline of existing turns, including active turns,
then records project enablement and exclusions. Repeating registration does not
reset that boundary. Turns without a start timestamp, or starting in/before the
registration second, remain excluded. This conservative second-resolution boundary
may omit a turn started immediately after registration; start work in the next second.
There is no implicit historical backfill.

`enqueue --codex-home PATH` reads a Stop/Interrupt hook object from bounded stdin.
A legacy `agent-turn-complete` notification can instead be passed as its positional
JSON argument. Only identities and cwd are stored from the event, not its prompt or
final answer. The event is a hint and cannot authorize extraction by itself. For
example, a legacy notify command array can point to:

```text
/absolute/coding-memory --state-dir /absolute/engine-state enqueue --codex-home /absolute/codex-state
```

Codex appends its JSON argument. Hook installation and trust approval remain manual;
the collector does not change normal Codex configuration or trust decisions.

Before starting generation in a dedicated Codex home, run
`own-store --codex-home PATH`. Owned stores and explicit native-run exclusions share
the same predicate for live delivery, reconciliation, and claims. The library also
supports excluding individual Run keys before generation. Changing observer or
runtime version does not create a new run identity.

## Identity and boundaries

Project identity is a SHA-256 of the canonical Git common-directory path. Linked
worktrees and branches share it. Separate clones are separate local projects, even
if their remotes match. Moving the common directory or deleting a worktree requires
an explicit migration/remapping; cross-machine identities are not provided yet.

A Run key contains canonical runtime-store identity, owning thread ID, and native
turn ID. Observer paths may differ between a signal and reconciliation, but the
project must agree. Immutable captured evidence records the confirmed per-turn cwd.

Reconciliation scans current worktree roots and the explicitly supplied project cwd
with `thread/list`, including archived threads. API cwd filters match exactly:
unknown nested session directories are not silently discovered. Invoke reconciliation
with the actual session cwd for such directories; live signals also accept registered
repository subdirectories. Deleted worktrees and unavailable sources are inspectable
failures, not a promise of recovery. A background scheduling loop is Issue #7.

The reader uses a separate stdio app-server and only initialize/read/list methods.
It never starts or resumes a development thread. Hooks, plugins, legacy notification,
and analytics are disabled for that invocation. Normal runtime configuration is
otherwise read: exec-only `--ignore-user-config`/`--ignore-rules` flags are not
supported by app-server. No model call is requested by this reader.

See [App capture diagnostics](app-capture.md) for detailed hold reasons and the
distinction between installed reader and thread creation versions.

Readers at or above `0.162.0-alpha.17.2` and creation versions at or above
`0.162.0-alpha.2` are admitted provisionally using SemVer precedence. Periodic local
checks and weekly Actions exercise the capture fixture; failed local checks block
new capture until revalidation passes. Admission never overrides required fields.
The selected executable must implement the experimental paging contract. Paged
responses are bounded to 8 MiB per request/traversal, 100 pages, and a 30-second
request deadline. Discovery also has an 8 MiB aggregate turn-metadata bound.

Turn completion requires persisted `completedAt`, a known terminal status, and
complete item pagination. A provisional interrupted status without completion
metadata is pending. Full native item objects retain message content, tool outputs,
call identities, terminal errors, timestamps, and evidence locators (bundle hash +
native item ID). The bundle is an API projection, not a complete raw transcript;
coverage explicitly identifies runtime truncation and unavailable attachment bytes.

A narrow local rollout metadata read confirms the selected turn's `turn_context`
cwd. This prevents attribution from a thread's current cwd alone. Artifacts outside
the explicitly selected Codex home are rejected. Rollouts larger than 64 MiB are held.
Conflicting per-turn cwd records are held. A fork prefix without a local turn context
is excluded, rather than recaptured under its child thread. Child/linked work and
compaction-only turns without a new human instruction are excluded. The original
rollout is not copied; immutable native item projections are the retained evidence.

## State and recovery

SQLite uses WAL, FULL synchronous writes, short transactions, a five-second busy
wait, and a versioned durable schema. Derived search work must use separate tables;
there is no command that resets this durable database. Unknown schema versions fail.

| State | Meaning |
| --- | --- |
| `queued` | Hint awaiting capture, including delayed persisted history |
| `capturing` | A token-fenced, expiring claim; no transaction remains open during reads |
| `ready` | Completed Run with persisted evidence, eligible for Issue #4 extraction |
| `recorded` | Failed/interrupted Run retained with evidence, ineligible for automatic extraction |
| `held` | Missing/unsupported/over-limit/ambiguous history, exclusion, or reader failure |

Claims are scoped to the selected runtime store. Their lease is one hour; an expired
claim can be reclaimed after restart, and stale workers cannot finalize it. Pending
history is rescheduled after ten seconds; after 60 attempts it is held as
`history_readiness_exhausted`. These are operational defaults, not measured latency
or throughput guarantees. `retry STORE THREAD TURN` deliberately retries a held run
with a fresh attempt budget. It cannot override owned-store or baseline exclusions.

Evidence is a content-addressed JSON file. A same-directory temporary file is written
and synced, replaced atomically, and the directory is synced before SQLite publishes
its digest. The engine state directory is private (0700), evidence files are 0600.
Repeated capture reuses identical content, and reads verify the digest. A crash before
state finalization can leave an unreferenced file or temporary file; it does not
publish incomplete evidence. Restart captures again and fences the old claim.
Reference-aware garbage collection belongs to Issue #5; do not delete evidence just
because a capture job finished. SQLite/evidence backups remain separate from the vault.

## Validation

Deterministic Rust tests cover duplicate routes, baseline and owned-store exclusion,
late exclusion, worktree identity, lease expiry/stale finish, store-specific claims,
explicit retry, immutable/hash-checked evidence, selected-turn tool evidence,
provisional interruption, failure outcomes, unsupported versions, read-only RPC
methods, repeated cursors, fork-prefix and ambiguous project contexts.

The installed CLI was also exercised with a fresh synthetic Git project, fresh
Codex state, and a loopback Responses provider. Without notifying the engine, a new
completed turn was reconciled, captured by Rust, and published as `ready`; repeating
reconciliation inserted zero duplicates. No personal history or paid model was used.
Repeat after building with:

```sh
nix develop --command python3 experiments/codex_capture/ingestion_probe.py \
  --engine /absolute/coding-memory/target/debug/coding-memory --codex /absolute/codex
```

The probe intentionally leaves its synthetic private evidence directory for inspection.
Real-project hook delivery and unattended scheduling are not enabled by this command.
