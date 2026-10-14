# Roadmap and GitHub Planning

Status: agreed planning baseline, approved on 2026-10-13 (Asia/Tokyo).
The three milestones and seven initial issues below were registered on
2026-10-13 (Asia/Tokyo). All seven issues belong to the Codex CLI MVP milestone.

See [design.md](design.md) for requirements and
[runtime-research.md](runtime-research.md) for evidence and unverified contracts.

## Milestones

| Milestone | Completion condition |
| --- | --- |
| [Codex CLI MVP](https://github.com/beso1225/coding-memory/milestone/1) | A CLI run becomes reviewed Markdown memory, and another session retrieves and uses it through MCP; automatic worker startup and recovery work |
| [ChatGPT App Daily Use](https://github.com/beso1225/coding-memory/milestone/2) | The same loop works for local Codex in ChatGPT App, with manual-edit protection and usable held-work handling |
| [DSH and Additional Backends](https://github.com/beso1225/coding-memory/milestone/3) | DSH capture works against its validated lifecycle/history contract, and at least one additional generation backend is evaluated |

Do not assign calendar deadlines before the capture and generation experiments
establish feasibility. Create detailed issues for the first milestone only;
retain later milestones as coarse scope until the preceding results are known.
Pi capture, semantic search, and graph retrieval remain an unscheduled backlog.

## Implementation sequence

1. Validate Codex CLI and App capture/history, and isolated Codex generation.
   App feasibility is an early gate even though CLI is the first complete adapter.
2. Bootstrap the Rust package and reproducible development environment. Implement
   durable capture/state, and build Vault-to-FTS-to-MCP retrieval using sample notes.
3. Implement evidence-grounded extraction into candidates.
4. Implement consolidation, separate review, semantic link proposals, and
   recoverable Markdown application.
5. Connect background operation and demonstrate CLI reuse across sessions.
6. Complete App daily use and its deferred manual-edit behavior.
7. Add DSH and evaluate additional generation backends using the same fixtures.

This moves retrieval ahead of a fully developed extraction pipeline so that
agent usefulness is tested early. Issue order below does not imply execution order.

## Registered initial issues

Use one repository (`coding-memory`) for software planning. Vault changes are
runtime data, not a second software issue queue. Keep setup work and related
substeps as checklists in these issues rather than creating a large issue tree.
Use milestone membership and explicit dependencies before adding more labels.

### [#1](https://github.com/beso1225/coding-memory/issues/1). Validate Codex CLI and App capture contracts

Problem: hooks, completion, and readable history are different boundaries, and
CLI behavior does not establish App behavior.

Deliver a small experiment report and synthetic fixtures, not the full adapter.
Record installed versions and the selected capture/history interfaces.

Acceptance criteria:

- CLI normal completion, steering, tool steps, failure, interruption, and Stop
  continuation have documented identifiers and outcomes.
- Notification/history ordering is measured or otherwise demonstrated; a later
  turn cannot enter an earlier run's evidence.
- Resume, compaction, and inherited history behavior are understood well enough
  to define the reader contract and fixtures.
- App hook execution and a viable App history route are checked separately.
  Unsupported or inaccessible routes are reported as such.
- The proposed reconciliation cursor is scoped to registered projects and an
  enablement boundary, with duplicate detection defined.

Dependencies: none. Treat lack of a feasible App route as a design checkpoint
before investing in a complete engine, since App is the daily-use target.

Progress: the [capture experiment report](codex-capture-experiment.md) records
isolated CLI and App-bundled executable probes plus offline synthetic fixtures.
The human-run App GUI probe also confirmed approved project-local hooks and a
cross-process cold read of the App-created synthetic thread. The capture
feasibility checkpoint is validated; App restart and broader daily-use behavior
remain later integration checks.

### [#2](https://github.com/beso1225/coding-memory/issues/2). Validate isolated Codex generation and review

Problem: Codex execution is a usable structured-output backend, but its tools,
hooks, configuration, and generated sessions can affect the capture pipeline.

Deliver an isolated experiment and output fixtures for the three generation
roles. Use synthetic evidence rather than personal session history.

Acceptance criteria:

- Fresh extraction, consolidation, and review contexts produce validated
  schema-shaped output, including an empty candidate result.
- Extraction/consolidation/review models can be configured independently.
- Output handling distinguishes success, timeout, process failure, malformed
  output, and a review that does not approve publication.
- Engine-owned runs are excluded from both notification capture and subsequent
  history reconciliation; ephemeral sessions alone are not the exclusion rule.
- Hook, legacy `notify`, tool, and MCP behavior is demonstrated for the exact
  execution settings. Unsupported isolation guarantees are documented.
- Project external-transmission policy is applied before a generation request.

Progress: [the generation isolation experiment](codex-generation-experiment.md)
records role-separated synthetic exec runs, output/policy tests, and the observed
remaining tool exposure. A dedicated policy hook denied command execution, and
three fresh authenticated contexts returned structured results. Broader tool
coverage and durable ownership remain implementation checks.

Dependencies: none; share identity findings with [#1](https://github.com/beso1225/coding-memory/issues/1).

### [#3](https://github.com/beso1225/coding-memory/issues/3). Implement durable ingestion and evidence capture

Problem: useful history must survive notification loss, duplicate delivery, and
worker restart before any generation or vault change occurs.

Bootstrap one Rust package with library/CLI entry points, a small Nix/direnv
development environment, and focused task/check commands. Implement project
registration, durable queue/state, the first Codex reader/adapter, and local
evidence capture against the validated contracts.

Acceptance criteria:

- Branches/worktrees of a repository resolve to a stable project identity.
- Duplicate live and reconciled signals identify the same run and do not create
  duplicate processing records.
- All terminal outcomes can be recorded, while only completed runs become
  eligible for automatic extraction.
- Captured history is bounded, preserves relevant tool evidence and locators,
  records coverage, and is reusable without rereading a mutable live session.
- Readiness delay, unsupported format, and lost history have distinct outcomes.
- Missed persisted runs can be reconciled; internal runs remain excluded.
- Interrupted claims recover without holding a database write transaction across
  long processing. Index rebuild cannot reset durable state.

Dependencies: [#1](https://github.com/beso1225/coding-memory/issues/1).
Coordinate the internal-run exclusion with [#2](https://github.com/beso1225/coding-memory/issues/2).

Progress: [durable ingestion](ingestion.md) implements the Rust CLI, SQLite queue,
project/worktree identity, scoped reconciliation, bounded cold-history reading,
and immutable evidence. Synthetic installed-runtime validation demonstrated
notification-free discovery and capture. Hook installation remains manual;
background scheduling belongs to #7. Exact-cwd discovery and local source/size
limits are explicit operational boundaries.

### [#4](https://github.com/beso1225/coding-memory/issues/4). Implement evidence-grounded extraction

Problem: a completed run may contain reusable knowledge or nothing worth saving;
generation errors and unsupported claims must not masquerade as successful memory.

Acceptance criteria:

- Extraction uses captured evidence and writes candidates, not Markdown.
- Empty extraction is successful and distinguishable from generation failure.
- Candidate shape, project/scope, conditions, and evidence locators are validated.
- Progress-only information and unsupported inferences are not automatically
  promoted into active memory.
- Backend/model and prompt/schema provenance are recorded.
- An externally disallowed project stays unprocessed/held rather than silently
  using an external fallback.
- Focused fixtures cover supported knowledge, no useful knowledge, invalid
  references, and retained applicability conditions.

Dependencies: [#2](https://github.com/beso1225/coding-memory/issues/2) and
[#3](https://github.com/beso1225/coding-memory/issues/3).

### [#5](https://github.com/beso1225/coding-memory/issues/5). Implement reviewed consolidation and recoverable Vault updates

Problem: candidates can duplicate, qualify, contradict, or replace existing
knowledge, and filesystem changes can be interrupted independently of SQLite.

Acceptance criteria:

- Consolidation triggers at five ready candidates or a ten-minute oldest-candidate
  age per project; immediate execution exists and empty/held batches do not loop.
- Proposals cover create, minor update, supersede, skip, and hold behavior.
- A fresh review context checks evidence, conditions, scope, conflict handling,
  supersession, and related links before publication.
- Review acceptance remains separate from verification state; an observed result
  is not generalized beyond its evidence.
- Rejected, dependent-on-rejected, invalid, or insufficiently supported operations
  are not written.
- One independent knowledge unit has one stable note identity. Superseded notes
  remain available with their successor and evidence references.
- Related-link candidates come from scoped retrieval, and generated Wikilinks
  target supplied, existing, accepted identities. Unclear relations are omitted.
- Durable change records, revision checks, and atomic per-file replacement support
  restart recovery without duplicate notes or premature candidate consumption.
- Tests interrupt application before/after a file replacement and before state
  finalization; unexpected content is held as a conflict.
- The MCP/index recovery path does not present affected entries as current while
  their application is unresolved. Whole-batch atomicity is not claimed.
- Referenced evidence is retained. Only unreferenced, no-value evidence becomes
  eligible for configurable cleanup after 30 days.

Dependencies: [#3](https://github.com/beso1225/coding-memory/issues/3),
[#4](https://github.com/beso1225/coding-memory/issues/4), and the search service from
[#6](https://github.com/beso1225/coding-memory/issues/6).

### [#6](https://github.com/beso1225/coding-memory/issues/6). Implement scoped FTS retrieval and MCP tools

Problem: the agent needs bounded, applicable memory results rather than a vault
dump, and retrieval usefulness should be established before extraction is complete.

Start with sample Markdown notes and a shared engine search/read service.

Acceptance criteria:

- Search data is rebuildable from Markdown without resetting durable processing.
- Default search includes active target-project and explicit global notes;
  other projects and historical notes require deliberate selection.
- Search returns short summaries, stable identities, conditions, and support/review
  metadata; read returns the selected authoritative note.
- Code identifiers, punctuation, paths, and empty/invalid query handling have
  focused fixtures. Search queries are constructed safely by the engine.
- A local stdio MCP server exposes read-only search/read functionality.
- Agent instructions describe when to search and how to form English queries;
  retrieval does not depend on an internal LLM translator.
- A Codex session demonstrably retrieves a known sample memory through MCP.
- Existing retrieval stays available while generation is busy or fails, with
  explicit handling of affected entries pending recovery/refresh.

Dependencies: storage/domain bootstrap from [#3](https://github.com/beso1225/coding-memory/issues/3);
extraction is not required.

### [#7](https://github.com/beso1225/coding-memory/issues/7). Integrate background operation and validate cross-session reuse

Problem: the end-to-end system must work in normal local development without a
manually managed foreground process or hidden processing failures.

Acceptance criteria:

- One worker starts at macOS login and can run in the foreground for development;
  duplicate worker ownership is prevented.
- Executable, state, and configuration locations work without an interactive
  shell environment.
- Status distinguishes empty extraction, pending work, generation failures,
  conflicts, and held/exhausted operations; deliberate retry is available.
- Runtime restarts, worker restarts, duplicate events, delayed history, and bounded
  generation retries meet the preceding recovery contracts.
- A completed CLI development run becomes a reviewed note and a separate session
  retrieves and applies the relevant knowledge through MCP.
- Representative examples assess extraction support, retained conditions, useful
  retrieval, and link validity. Report weaknesses instead of declaring quality
  from JSON validity or reviewer confidence alone.
- Worker operation does not commit or push the vault automatically.

Dependencies: [#1](https://github.com/beso1225/coding-memory/issues/1),
[#2](https://github.com/beso1225/coding-memory/issues/2),
[#3](https://github.com/beso1225/coding-memory/issues/3),
[#4](https://github.com/beso1225/coding-memory/issues/4),
[#5](https://github.com/beso1225/coding-memory/issues/5), and
[#6](https://github.com/beso1225/coding-memory/issues/6).

## Validation strategy

Use TDD for behavior changes where a focused regression test is practical.
Separate deterministic tests from provider/runtime experiments:

- Deterministic: decoding fixtures, run boundaries, deduplication, queue recovery,
  scope filters, schema/locator checks, Markdown serialization, retention
  references, and interrupted file application.
- Runtime: installed CLI/App event behavior, history readiness, generation
  isolation, MCP invocation, launch environment, and later DSH persistence.
- Knowledge quality: representative supported/unsupported/no-value evidence,
  preserved conditions, review decisions, expected retrieval, and useful links.

Keep synthetic fixtures small and versioned. Evaluate an initial representative
corpus before enabling unattended use on registered personal projects. Corpus
size, quantitative quality thresholds, retry limits, and resource ceilings should
be selected from the experiments rather than presented as already measured facts.
Record those settings when chosen; they are tunable operational parameters.

## Later scope

For App daily use, implement the validated App route, manual-edit detection with
subsequent automatic changes held, and a usable inspection/resolution flow for
held work. Revalidate bundled versions when the App changes.

For DSH, prefer a small host bridge over prematurely reimplementing its compressed,
versioned persistence. Validate terminal events, flush/readiness, boundaries,
fork/restart behavior, and independent generation configuration.

Compare additional generation backends against the same evidence and quality
fixtures. Ollama Cloud capability must be checked independently of Local. Local
model speed is not a blocking requirement for this project's first useful version.

Create later detailed issues from these results rather than expanding all future
work into tickets now.
