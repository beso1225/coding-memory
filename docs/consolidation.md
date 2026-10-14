# Reviewed consolidation and recoverable Vault updates

Issue #5 connects ready extraction candidates to proposed operations, a fresh
review context, Engine validation, durable application records, and Markdown.
The CLI runs one bounded batch per invocation. Automatic polling/startup belongs
to Issue #7; generation never edits the Vault itself or commits/pushes its Git repo.

## Foreground commands

Use an independently authenticated generation home and project transmission
policy as described in [extraction.md](extraction.md). Substitute your paths/ID:

```sh
coding-memory --state-dir /path/to/state consolidate \
  --vault /path/to/vault --project-id PROJECT \
  --codex-binary /absolute/path/to/codex \
  --model gpt-6.1-sol --review-model gpt-6.1-sol
coding-memory --state-dir /path/to/state consolidation-status
```

Default eligibility is five ready candidates or a 600-second oldest-candidate age
per project. `--min-ready` (1–20) and `--max-age-seconds` (1–86400) configure the
trigger. `--force` requests immediate work. Empty batches make no model call.
Only ready candidates are selected, at most twenty; held/reserved/consumed
candidates do not repeatedly trigger work. One generating/applying/conflicting
batch per project prevents overlapping ownership of its candidates.

The proposer and reviewer can use different models. They always use different
fresh Codex contexts and attempt directories, with the existing deny hook and
Engine-owned store exclusion. Permission is checked before generation and again
before review/application planning. Evidence from global notes also requires
the source project's transmission permission; no disallowed source is silently
sent via a permitted neighboring project. Combined input is bounded at 256 KiB
per call and output at 64 KiB. Oversized/unavailable context stays held.

## Proposal, review, and identity

Version-1 proposals assign every selected candidate exactly once to create,
update, supersede, skip, or hold. Minor updates retain identity, creation time,
conditions, and evidence. Supersession creates a successor and updates the old
note's status/successor while preserving its original evidence. Skip requires a
supplied existing target; unresolved contradictions use hold. The Engine derives
new IDs from the durable batch and operation ID and persists the exact intended
content before file writes. Application recovery never regenerates an ID.

The review input includes captured evidence, candidates, scoped existing notes,
and the entire proposal. A complete decision set names each operation and
accepts/rejects/holds it with a reason. Invalid review output authorizes nothing.
Rejected or held operations and dependencies on them do not publish. Operation
dependencies must refer to earlier operations, avoiding dependency cycles.
Review acceptance remains separate from observed/user-stated verification.
All supporting claims must retain the same verification category as the draft;
mixed categories must be separated or held. Minor updates also retain the
existing note's category. Unrelated observed evidence cannot upgrade a
user-stated claim. A successor describes newly supported knowledge while its
predecessor retains its original category.

Independent Engine checks enforce scope, exact supplied evidence locators,
eligible verification, retained conditions, allowed targets, and note schema
bounds. They cannot prove a semantic inference, useful granularity, or valid
general applicability. Fresh model contexts can still share correlated mistakes.
Separate semantic review is required but is not a proof of correctness.

Existing/related candidates come from scoped FTS retrieval (up to three hits per
candidate title, ten distinct existing notes total). The model proposes a relation;
the reviewer checks it. Links must name supplied accepted notes. The Engine
renders `[[path-without-extension|stable-id]]`, accommodating existing filenames
that differ from IDs. Draft bodies cannot inject their own Wikilinks. Ambiguous
relations should be omitted. Lexical title retrieval can miss duplicate/related
knowledge; semantic recall and actual model quality remain evaluation work.

## Durability, conflicts, and recovery

Batch status distinguishes generating, held, abandoned, applying, conflict, and
applied, and exposes review decisions. An applied batch can have no accepted
publication: rejected/held candidates stay held and do not loop. Generation or
validation failure also holds candidates, with a bounded reason and persisted
available provenance/context. Retrying held or expired generation is explicit:

```sh
coding-memory --state-dir /path/to/state consolidation-retry BATCH
coding-memory --state-dir /path/to/state consolidate \
  --vault /path/to/vault --project-id PROJECT --force
```

A generating lease lasts one hour. An expired batch is not automatically reused;
retry abandons it and releases its candidates for a new proposal/review. An old
worker cannot save a plan into the abandoned batch. Generation calls hold no
SQLite write transaction. Transient model failures currently use deliberate
batch retry, rather than an automatic unbounded review loop.

Reviewed plans persist paths, before/after hashes, exact content, candidate
consumption, and per-note publication barriers in one SQLite transaction. Related
and skip targets receive revision guards as well. For each file, application
compares current content with the expected before/after state, writes a temporary
file on the same filesystem, syncs it, atomically replaces the target, syncs the
parent directory, and records progress. Short SQLite write transactions serialize
Engine file application; they never span model calls.

After interruption, unchanged before-state is applied and matching after-state is
recognized without creating another note. Unexpected content becomes a conflict.
Resolve it by restoring a reviewed before/after state, then recover the same batch:

```sh
coding-memory --state-dir /path/to/state vault-recover --vault /path/to/vault BATCH
```

The recorded Vault must match. Recovery cannot force-overwrite unexpected content.
Intended revisions/content are retained in the local `vault_changes` table for
inspection. Index refresh and final hash checks precede atomic candidate
consumption and barrier release. A separately running MCP process checks those
barriers, even after an independent index rebuild, and cannot expose affected
notes as current while application is unresolved. Unaffected notes remain usable.

Per-file atomicity is not whole-batch atomicity: Obsidian can observe intermediate
files. Concurrent human-edit merging remains deferred; preconditions detect stale
plans but do not promise filesystem compare-and-swap against arbitrary editors.
Durability uses file/directory `sync_all` and SQLite WAL/FULL. Interruption tests
exercise recovery boundaries, not hardware power-loss guarantees.

## Evidence retention and validation

Cleanup is disabled unless explicitly requested. Preview or opt into deletion:

```sh
coding-memory --state-dir /path/to/state evidence-cleanup \
  --vault /path/to/vault --retention-days 30
coding-memory --state-dir /path/to/state evidence-cleanup \
  --vault /path/to/vault --retention-days 30 --apply
```

Only successful empty-extraction evidence at least thirty days old can qualify.
All candidate references and active/superseded note references are retained.
Invalid notes or unresolved publication prevent cleanup. Longer retention is
configurable; deletion is never part of normal consolidation. Existing candidate
and generation artifacts are conservatively retained rather than treated as
disposable no-value evidence.

```sh
nix develop -c pkf run check
nix develop -c python3 experiments/codex_generation/extraction_probe.py \
  --engine target/debug/coding-memory --codex /absolute/path/to/codex --consolidate
```

Offline tests cover all operation kinds, scope/condition/evidence rejection,
rejected dependencies, link paths and guards, review rejection/malformed output,
permission gating, count/age scheduling, interrupted application before/after
replacement and before finalization, unexpected content, MCP barriers, and
retention. A synthetic loopback probe on Codex CLI 0.162.1 demonstrated capture,
extraction, fresh consolidation/review, publication, and retrieval with preserved
conditions and available evidence. No normal credentials, login, personal history,
or paid-model call were used. This verifies integration, not real extraction or
review quality; representative quality and cross-session application remain #7.
