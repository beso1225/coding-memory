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
per call — the complete prompt plus the serialized response schema, the same
budget extraction uses — and output at 64 KiB. Oversized/unavailable context
stays held.

## Referenced evidence projections (Issues #20 and #60)

Consolidation verifies the hash, schema and completed outcome of each immutable
original source before selecting items. The transient model context starts from the
items referenced by the selected candidates or retrieved existing notes, deduplicated
by original bundle hash and native item ID. Selected items retain their full JSON
and original order; source metadata is preserved. Missing or ambiguous referenced
IDs fail closed. Both proposer and reviewer receive the same projection. Engine
validation rejects citations to omitted items, even if those items exist in the
original file.

For a cited DSH execution result (`ds_type: "tool/result"`, normalized to
`commandExecution`), the Engine also includes the request that records the executed
command and arguments (`ds_type: "tool/call"`, normalized to `toolCall`). The match
uses the native call identity: the request's `data.callId` equals the result's
`data.message.toolCallId`, then `data.toolCallId`, then `data.id`, matching the
capture adapter's accepted result-identity fallbacks. Pairing is strictly
within one immutable bundle and applies to candidate and retrieved-note references
alike. Explicit requests, requests shared by several selections, and repeated
candidate/note references are not duplicated; expanded items remain in their original
source order. If the call identity is missing or malformed, more than one
well-formed request in the bundle claims it, or the matching request does not have a
unique item ID, only the explicitly selected items are projected: the Engine never
guesses a request from another item, turn, run, or bundle. Non-DSH evidence is
unchanged.

Automatically included requests are supplied evidence and may be cited in a draft
like any other projected item. They do not upgrade verification: `observed` still
requires a completed execution item, and a `toolCall` request alone is not observed
support. If a draft cites an automatically included request, the rendered note
retains that reference. A note that stores only the result still re-expands to the
request when it is selected later, because the source bundle is immutable and the
relationship is deterministic. Preflight, partitioning, generation and independent
review all measure the expanded projection, including the added request bytes. The
selected-context diagnostic continues to report explicit referenced-item and bundle
counts, not a count of automatically added support. Extraction citation guidance is
separate: a candidate may cite the result alone or both items, and this projection
supplies the matching request either way.

Each model evidence entry carries `projection_schema_version: 1` and
`source_bundle`, the hash of the immutable original. The projection is not a new
evidence artifact and is never saved over the original. References in published
notes continue to use the original bundle hash and native item ID. Consolidation
and review keep their existing prompt contracts (`consolidation-v3` and
`review-v2`): automatically added requests are ordinary supplied evidence under
the existing citation rule.

Local source reads have a separate 16 MiB per-file cap, allowing captured items
(up to 8 MiB) plus their JSON envelope to be verified without treating them as
model input. This cap bounds source parsing; it does not increase the 256 KiB
prompt-plus-schema limit or the 64 KiB output limit. Source project transmission
permissions remain required, including for retrieved global notes.

## Serialized prompt budget (Issue #20, second increment)

Before either model runs, the Engine measures the complete serialized
consolidation prompt, including instructions, JSON escaping, candidates,
existing notes and projected evidence, plus the serialized consolidation
response schema. Its initial target is 184 KiB: the 256 KiB hard cap less the
64 KiB proposal reserve and an 8 KiB margin. The Engine also measures the review
framing/context and reserves 64 KiB for the proposal plus the review schema,
whose operation identities are budgeted at the accepted maximum of 20 operations
of 128 bytes each. It checks that the resulting review request fits the 256 KiB
hard cap as well, so a context is selected only when both measurements fit. Both
measurements use the same prompt builder and schemas as generation. The final
check before each call measures the same sum — complete prompt plus serialized
response schema — that the generation backend re-checks, so a request over the
256 KiB budget is held as unavailable context before any model runs. The actual
proposer output remains capped at 64 KiB.

The local generation provenance records `input_budget` with measured
`consolidation_bytes`, `consolidation_target`, `review_max_bytes`, `input_limit`
and `proposal_limit`, all counted the way the request is counted, including the
response schema. These size fields contain no source text. New batch records
also expose typed `diagnostics` through `consolidation-status`: the selected
context, individually oversized contexts, deferred count and any preparation
error. Each context reports candidate IDs/count, related-note count, deduplicated
referenced-item/bundle counts, budget and bounded reason/detail codes. Counts or
budgets that could not be computed are `null`, not zero. Older records without
these fields remain readable and omit `diagnostics`; use preflight to measure
such held work against the current Vault. The local `partition` record retains
selected/oversized candidate IDs and the deferred count.

Candidates are examined in stable `created_at, id` order, up to the existing
reservation limit of twenty. The Engine adds candidates while their complete
context fits. A candidate that cannot fit alongside the selected prefix is also
checked alone, with its full retrieved comparison context. If it fits alone,
selection stops and that candidate plus the unexamined suffix return to `ready`
for a later invocation. Their original creation times remain intact, preserving
the count/age trigger. If it cannot fit alone, only that candidate is held as
`held_context_too_large` and selection can continue with later candidates.
Evidence items are not truncated, and retrieved neighbors are not removed to
force a candidate through the budget. Both rules are deliberate;
[roadmap.md](roadmap.md) records the bounded alternatives considered if sizing
holds recur.

The reservation update is one lease-checked SQLite transaction. Selected
candidates remain reserved; deferred candidates lose their reservation links
and return to `ready`, so size-only deferrals do not count as generation attempts.
Individually oversized candidates receive separate held jobs that can be
explicitly retried. When none fit, the parent job holds the first oversized
candidate and no model is called. These sizing holds also do not consume the
three-attempt budget for later transient model failures. Worker recovery reclaims
only the selected reservation, preserving deferred and separately held work.
One invocation still generates at most one proposal and one independent review;
the normal worker loop or subsequent foreground invocations process the rest.

Missing/invalid evidence remains `held_context_unavailable`; this change does not
silently treat it as an oversized valid source. The final hard-cap guard also
retains that reason if it detects an unexpected prompt-size failure.
Extraction input budgeting and resident-memory improvements remain separate
work. Existing held batches are not automatically retried or discarded. After
installing the paired-request projection, inspect a held batch with
`consolidation-preflight`, then deliberately retry only the batch whose command
evidence is now supplied with `consolidation-retry BATCH`. The expansion happens
when context is prepared again, so a retry sees the request without rewriting
stored candidates, notes, or immutable evidence. Do not bulk-retry or rewrite
held candidates.

## Held-batch preflight (Issue #20, final increment)

Inspect explicitly selected held/exhausted batches before retrying them:

```sh
coding-memory --state-dir /path/to/state consolidation-status
coding-memory --state-dir /path/to/state consolidation-preflight \
  --vault /path/to/vault --project-id PROJECT --batch 6 --batch 7
```

The command accepts 1–20 distinct positive batch IDs belonging to the requested
project, with 1–20 currently held, non-excluded candidates per batch. Processing,
applied, abandoned, foreign-project, empty and excluded batches are rejected.
It uses an existing read-only state connection, creates no directories or schema
in the state root, and builds a fresh private search index in an automatically
removed temporary directory. The real search index, queues, candidates, evidence,
Vault and generation credentials are not changed. No Backend, Codex process,
login, model call or network request is involved. The state database uses
write-ahead logging, so read-only access may create or reuse SQLite's `-wal` and
`-shm` coordination files in the state root while another connection holds the
database open; no schema, rows or lasting state are written.

By default preflight checks the same external-transmission permissions as Codex
consolidation, including each evidence origin project. `--local-only` checks the
local-generation case without those external permission gates; it still calls
no model. It is an assumption for the report, not a change to stored policy.
Publication barriers and default project/global neighbor scope remain enforced.

The JSON report contains `schema_version`, `project`, `assumes_external`,
`snapshot_only`, `invalid_note_count`, and `batches`. Each batch reports proposed
`partitions` and a preparation `error`, if any. Partitions use the same selection
and prompt builder as normal consolidation and include the diagnostics described
above. Error details distinguish missing, corrupt, unsupported or oversized local
source files, missing/ambiguous native locators, and policy denial without
returning source text, filesystem paths or raw parser/process errors. Oversized
valid inputs report their full serialized prompt size; counting writes to a byte
counter rather than allocating an oversized prompt. At most one original source
is parsed at a time and retained projected evidence stays within the hard cap.

This is a current-state estimate, not an approval or an executable plan. Preview
normalizes candidate state to `ready` as a retry would and numbers hypothetical
partitions from the same next batch ID for every requested batch, advancing only
across that batch's own partitions; it does not reserve those IDs or simulate
model decisions, new Vault notes or cross-batch regrouping. Concurrent worker
activity, permission changes, new publications and resulting neighbor retrieval
can change later partitions.
Invalid notes are counted and excluded by the normal index rules. Recheck if the
Vault or queues change; normal generation always repeats all validations.

Review the report, then deliberately retry only the batch whose cause is resolved:

```sh
coding-memory --state-dir /path/to/state consolidation-retry 6
coding-memory --state-dir /path/to/state consolidate \
  --vault /path/to/vault --project-id PROJECT --force \
  --codex-binary /absolute/path/to/codex
```

A running worker may claim the released candidates before the foreground command.
Preflight never releases them itself. An individually oversized required context
continues to hold; no evidence truncation or deletion is authorized by a preview.

## Proposal, review, and identity

Version-1 proposals assign every selected candidate exactly once to create,
update, supersede, skip, or hold. Minor updates retain identity, creation time,
conditions, and evidence. Supersession creates a successor and updates the old
note's status/successor while preserving its original evidence. Skip requires a
supplied existing target; unresolved contradictions use hold. The Engine derives
new IDs from the durable batch and operation ID and persists the exact intended
content before file writes. Application recovery never regenerates an ID.

The proposer contract makes candidate applicability explicit: every operation's
draft must copy, verbatim, the conditions, verification category and evidence of
each candidate it consumes, and a summarized, reworded, merged or dropped
condition, category or reference is rejected. Because a targetless `skip` had
recurred, `skip` must name a supplied existing note to compare against, and when
there is no supplied existing note the only fallback is `hold`, which publishes
nothing. The response schema binds each action to the target and draft shape
`validate` accepts, so a `skip` cannot be requested without a supplied target and
a `create` cannot name an existing one by construction; with no supplied note,
`update`, `supersede` and `skip` are not offered at all. This contract is a
proposer obligation: the consolidation prompt is `consolidation-v3`, while the
independent review prompt is deliberately unchanged and stays byte-identical to
`review-v2`.

A single invalid operation no longer parks the whole batch: every decode or
`validate` failure, mechanical or semantic, records the rejection in
`proposal_repair` and the Engine re-requests the proposal once, echoing the
rejection into the input's `repair` object; the repair attempt uses its own
attempt directory. The record is written as soon as a proposal is rejected, so a
repair that exits early (revocation, timeout, over-limit prompt) still leaves the
trigger inspectable: `attempts` counts the proposals generated, `echoed` is the
rejection sent back to the model, and `final` is the rejection that held the
batch when validation was exhausted. Because the repair transmits the same
evidence a second time, the external policy is re-authorized before every
attempt; a revocation stops the next send and holds the batch as
`held_external_policy`. `validate` remains the authority: a repair that still
fails, a second decode failure, or an over-limit repair prompt holds the batch
with `held_invalid_proposal` exactly as before, so one defect cannot silently
discard the batch's other candidates, loop unboundedly, or publish an unvalidated
draft. Held batches stay inspectable and deliberately retryable.

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

Existing/related candidates come from bounded scoped FTS retrieval. In addition
to the first eight searchable title chunks, the Engine searches individual terms
from title (up to four), claim (up to three), and applicability conditions (up to
one). The title query retains short words, including titles such as `Use Go`.
Supplemental terms are lowercased; common English function words, tokens shorter
than three bytes and terms exceeding the 1024-byte query limit after normalization
are omitted without truncating identifiers. Duplicate terms are removed before
each field quota is counted, so a claim repeating its title can still contribute
new terms. Identifier/path punctuation is retained. Repeated
queries are cached within the batch. Each query returns at most five hits, with at
most nine searches per candidate and 180 per twenty-candidate batch.

Results are ranked per candidate by reciprocal-rank fusion: each query contributes
`1_000_000 / (60 + zero_based_rank)`, with stable-ID tie breaking. Raw BM25 values
from different queries are not added. The top three neighbors per candidate are
combined round-robin into at most ten distinct existing notes, giving later
candidates space before earlier candidates receive their second/third neighbors.
The same existing-note context reaches the proposer and independent reviewer.
Project/global scope, historical exclusion, revision checks, publication barriers,
and combined evidence/prompt limits still apply. No additional model call is used.

The model proposes a relation; the reviewer checks it. Links must name supplied
accepted notes. The Engine renders `[[path-without-extension|stable-id]]`,
accommodating existing filenames that differ from IDs. Draft bodies cannot inject
their own Wikilinks. Ambiguous relations should be omitted.

This improves paraphrases sharing technical terms; it does not provide embeddings,
synonyms, stemming, Japanese segmentation or guaranteed semantic recall.
Common-term hit limits and field quotas can still miss neighbors. Retrieval alone
does not authorize merging independent knowledge or prove a model will deduplicate
it correctly. The public CLI/MCP search contract remains literal AND; only
consolidation performs these multiple searches.

Issue #15's offline fixtures reproduce a legacy title-query miss, retrieve
paraphrased/related notes, preserve scope and availability boundaries, exercise
identifier queries/rank fusion and bound per-candidate/batch results. Scripted
proposal/review integration covers update, skip and distinct related creation
without private histories or paid-model requests. Personal Vault cleanup and
test-example Wikilinks in frontmatter are separate follow-up concerns.

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
