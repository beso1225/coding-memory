# Evidence-grounded extraction

Issue #4 implements foreground extraction from immutable captured evidence into
SQLite candidates. Extraction does not publish Markdown. A `ready` candidate has
structurally eligible evidence; it is not semantically reviewed or exposed through
MCP. [Consolidation](consolidation.md) implements the subsequent independent
review and recoverable Vault application from Issue #5.

## Authentication and commands

Use the same private state directory and project ID as ingestion. Substitute
your paths and project ID below; state must stay outside both repositories.

```sh
coding-memory --state-dir /path/to/state generation-setup
CODEX_HOME=/path/to/state/generation/codex-home \
  codex -c 'cli_auth_credentials_store="file"' login
coding-memory --state-dir /path/to/state external-policy --project-id PROJECT allow
coding-memory --state-dir /path/to/state extract --project-id PROJECT \
  --codex-binary /absolute/path/to/codex --model gpt-6.1-sol --timeout-seconds 300
coding-memory --state-dir /path/to/state extraction-status --project-id PROJECT
coding-memory --state-dir /path/to/state candidates --project-id PROJECT
coding-memory --state-dir /path/to/state external-policy --project-id PROJECT deny
```

Setup creates an Engine-owned home, installs its deny hook, and marks the store
excluded before generation starts. Login is an explicit human operation. The
Engine never reads or copies regular Codex credentials, requires its own regular,
nonsymlinked `auth.json`, and removes inherited API-key overrides for generation.
Keep credentials private and out of Git. Refresh/expiry is an operational concern,
not a reason to borrow another runtime's credentials. The initial
[quality evaluation](quality-evaluation.md) exercised authenticated generation
with synthetic evidence and an independently logged-in evaluation home.

External transmission defaults to denied per project. The gate precedes Backend
invocation, home preparation, and authentication checks. Allowing transmission
requeues policy-held jobs. Revocation prevents subsequent requests and staging
after an in-flight request completes; it cannot undo a request already sent.
There is no external fallback.

`extract` enqueues completed captured runs once and drains currently eligible
jobs. Deferred retries need a later invocation or the [background worker](worker.md).
`extraction-retry STORE THREAD TURN` explicitly requeues a held
or exhausted job. Inspect and fix its reason first. Existing jobs held as
`held_invalid_or_oversized_evidence` are preserved; deliberately retry them after
upgrading to use bounded extraction. Upgrading does not automatically transmit them.

## Candidate and persistence contracts

The closed version-1 batch carries role `extraction`, source project ID, and zero
to twenty candidates per chunk. Each records kind (`failure_solution`, `procedure`,
`constraint`, or `design_decision`), English title/claim, applicability conditions,
project or explicit global scope, verification (`observed`, `user_stated`, or
`unverified`), and exact supplied evidence digest/native item IDs.

Unknown fields/kinds, wrong project assignments, foreign/missing locators,
oversized text, and identical candidates within one call reject the whole batch.
Observed eligibility requires a referenced completed tool item; user-stated
eligibility requires a user message. Other support stays held. These mechanical
checks cannot establish whether the claim follows from the text or a global
generalization is justified. The prompt excludes progress and speculation; all
candidates still require independent semantic review before publication.

Job states distinguish `succeeded`, `succeeded_empty`, `held`, retry `queued`, and
`exhausted`. Invalid evidence, indivisible oversized input, invalid candidates,
missing credentials, unsupported runtimes, and tool attempts hold the job. Timeout/process failures
retry after ten seconds for at most three attempts. Repeated expired leases also
exhaust after three attempts. No failure becomes an empty success. Candidate
insertion and completion commit atomically behind a lease token; stale or
collector-excluded work cannot stage candidates.

Provenance records backend/provider/model, role, Engine and prompt/schema versions,
evidence digest, and input/schema hashes. Returned Backend output adds output
hash, native generation thread ID, and runtime version. Failures retain available
provenance and bounded reasons without recording raw stderr or printing prompts.
Native sessions and generation artifacts remain private local state; retention
policy is later operational work.

## Execution boundary and validation

Codex executes in a fresh context/directory per attempt, ignores user configuration
and rules, disables plugins and legacy notification, and uses a read-only sandbox
plus a dedicated `PreToolUse` deny hook. The hook always exits with denial, even
for malformed input, and stores only a hashed session identity rather than tool
contents. Detected tool attempts reject the batch. Engine-owned exclusion applies
to both live capture and reconciliation. Symlinked generation directories,
policy, and output are refused.

Immutable source evidence is read locally up to 16 MiB, hash-checked and validated
before generation. Every item is assigned once to a contiguous chunk in source
order. Each call budgets at most 256 KiB for the complete prompt (instructions,
metadata, digest, JSON escaping and delimiters) plus serialized response schema.
Native IDs and original evidence bytes are preserved. The schema and validator
permit citations only to items supplied in that call, even if an omitted ID
exists in the original bundle. Missing context cannot support an inferred claim;
facts requiring context across chunks may remain unextracted and need independent
review rather than invented support. Metadata is repeated without truncation.

All chunks must succeed before candidates and job completion commit in one
transaction. Chunks share no item and every candidate cites at least one supplied
item, so two calls cannot produce the same candidate; merging is a defensive exact
check rather than a deduplication guarantee. Distinct citations, verification or
applicability conditions remain distinct for consolidation/review, and candidates
that overlap semantically are deliberately left for review rather than merged.
The combined result can exceed twenty candidates, with the twenty-candidate and
64 KiB output limits applying separately to each call. A failed chunk stages
nothing. Automatic or deliberate retries restart all chunks; successful chunks
are not checkpointed. Requests have unique attempt/token/chunk identities.
Lease renewal between calls fences expired or superseded work, and external
permission is rechecked before each call and at the final commit.

Evidence holds distinguish `held_missing_evidence`, `held_invalid_evidence`
(including corruption, unsupported schema or mismatched identity),
`held_oversized_evidence_source` (local 16 MiB ceiling),
`held_oversized_evidence_framing` (metadata/framing cannot fit), and
`held_oversized_evidence_item` (one complete item cannot fit). No item is silently
truncated or omitted, and all chunks are planned before the first call.
`held_external_policy` remains a separate transmission-policy decision.
Diagnostics record source bytes/limit, input bytes/limit, item/chunk counts,
completed chunk count and current chunk index (zero-based), without source text
or model stderr. Input/schema/output hashes and generation-thread fields describe
the latest attempted chunk; output fields are cleared before another call.

Final output is capped at 64 KiB per call, exec stdout at 1 MiB, and exec stderr
at 256 KiB. Timeout is configurable from one to 600 seconds. Timeout
or bounded-I/O failure kills the process group owned by that invocation. Supported
generation versions remain exactly gated to `0.162.1` and `0.162.0-alpha.17.2`.
Capture admission and its periodic fixture do not validate tool-denial isolation
for newer generation runtimes.
The version probe separately caps stdout at 4 KiB and stderr at 16 KiB. Invalid
CLI timeouts are rejected before opening state or claiming work. Policy holds
recheck permission inside their state-transition transaction, so a concurrent
allow cannot strand a job. Captured tool events and denial markers take priority
over process failure or timeout, preventing a denied attempt from being retried.

Read-only execution still exposes tools. The tested deny hook does not prove
that future tool families, managed configuration/hooks, or all filesystem/network
access are isolated. Revalidate supported runtimes before sensitive unattended
use. The Backend interface separates transmission classification, provenance,
and generation from validation/persistence for later providers.

```sh
nix develop -c pkf run check
nix develop -c python3 experiments/codex_generation/extraction_probe.py \
  --engine target/debug/coding-memory --codex /absolute/path/to/codex
```

The opt-in probe creates isolated synthetic source histories and a loopback
Responses provider. Codex CLI 0.162.1 demonstrated denied transmission before home
preparation, supported knowledge with retained conditions/locators, empty success,
actual command denial without its sentinel file being created, exclusion of
persisted generation sessions from reconciliation, and an oversized synthetic
bundle extracted through two calls with distinct attempt identities and per-chunk
schemas. A chunk citing an item outside its own part is held with nothing staged.
It performs no login or paid-model call. This verifies wiring and controls, not
actual LLM quality.
Offline tests cover malformed output, tool attempts, timeout/process failure,
invalid scope/locators, held support, permission gating, lease recovery, bounded
retries, directory protection, and transactional rollback.
