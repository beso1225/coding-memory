# DeepSeek Harness capture experiment

Issue: [#56](https://github.com/beso1225/coding-memory/issues/56).
Implemented on 2026-10-20 (Asia/Tokyo). This report records what was run, not
what DSH source appears to promise.

## What was run

- Runtime: local DSH checkout at `0.2.1-rc.2`, built artifacts, launched as
  `node apps/cli/lib/bin.js --profile headless --json "<task>"`. The TypeScript
  source entry (`tsx apps/cli/src/bin.ts`) fails in this checkout with a Cordis
  export mismatch, so only the built entry was used.
- Engine: this repository at `target/debug/coding-memory`.
- Isolation: [experiments/dsh_capture/run.py](../experiments/dsh_capture/run.py)
  creates a fresh directory under the OS temporary area with a synthetic Git
  project, a private `DSH_HOME`, a private engine state directory, and a
  `--patch` overlay that mounts the bridge. No personal `DSH_HOME`, transcript,
  credential file, or network route is used by the harness itself.
- Model calls: opt-in through `--provider`. Two passes were recorded:
  - Uncredentialed: every turn ended
    `{"kind":"error","error":{"code":"MISSING_CREDENTIAL"}}`. That is a real,
    complete turn on the same logging, `session/event`, and flush path.
  - Credentialed: route `ollama-cloud`, model `deepseek-v4.1-flash:1017`, one
    plain-reply turn and one tool-using turn (`read` + reply), each ending
    `{"kind":"completed"}`. The isolated `DSH_HOME` received a copy of the
    credential store for the duration of the run only; it was deleted
    afterwards.

## Credentialed results

| Run | Turn | End | Captured | Evidence items |
| --- | --- | --- | --- | --- |
| baseline, plain reply (the first session of the script) | 1 | `completed` | `ready` after registering that session; later registration excludes it as the baseline | — |
| after, plain reply | 1 | `completed` | `ready` | `user/message`, `assistant/message` |
| after, tool use | 1 | `completed` | `ready` | `user/message`, `assistant/message`, `tool/call`, `tool/result`, `assistant/message` |

Real completed turns added types the failed-turn probe never showed:
`system/message` (the rendered system prompt), two `session/title` records with
`session/title-llm-request`, a second `step` when a tool runs, and settled
`assistant/attempt` records. The first run exposed two projection defects that
are now fixed and regression-tested:

- `system/message` was unknown and held the whole turn as
  `unknown_session_event_type`. It is bookkeeping: a recomposed prompt carries no
  fact about what the human asked.
- `assistant/attempt` was projected as an item. It now only contributes to the
  size bound, because the committed `assistant/message` is the evidence and the
  attempt's embedded stream is never rendered.

With the fixes, `tests/dsh.rs` replays the exact record sequence of a real
completed headless turn, so a future vocabulary drift fails a test instead of
silently holding production runs.

## Credentialed runs also showed

- Usage is logged per step (`inputTokens`, `outputTokens`, `cacheReadTokens`), so
  a captured turn's cost is attributable.
- The headless profile disables the managed credential store and authenticates
  from the launch environment only. An isolated home needs
  `- id: credentials / disabled: false` in the overlay; the experiment script
  adds it, exactly as the headless README documents for stored credentials.
- The credential store mounts at `$DSH_HOME/.credentials.yaml`; a copy under any
  other filename is ignored.

## Verified behavior

| Question | Observed |
| --- | --- |
| Is `turn/end` recorded for a failed step? | Yes. The log ends `step/end` → `turn/end {turn: 1, reason: {kind: "error", error: {code: "MISSING_CREDENTIAL"}}}`. |
| Does `turn/end` fire on `session/event`? | Yes, in the same process, after the event is committed to the live log. |
| Is the persisted log complete when the process exits? | Yes. The headless runner awaits `sessions.flush(agent.session)` before exiting, so a reader started afterwards sees the whole turn. |
| Is the persisted log complete *at* `turn/end`? | Not guaranteed. `session/event` is fire-and-forget and `turn/end` does not wait for persistence. The bridge therefore never reads at `turn/end`; it only writes a completion hint and lets the engine read the session. |
| What does a user-role event look like? | `user/message` events carry `source.kind`. One headless turn produced four user-role events: one `user` and three harness-injected context messages. A long interactive session showed the same pattern (`time-context`, `agent-instructions`, `runtime-context`, `skill-catalog`). A human run is identified by `source.kind === "user"`, never by role or count. |
| What event vocabulary appears? | `permission/preset`, `sandbox/mode`, `approval/policy`, `agent/inbox/spliced`, `turn/start`, `step/start`, `system/message`, `user/message`, `request/header`, `request/context`, `session/title`, `session/title-llm-request`, `assistant/attempt`, `step/end`, `turn/end`, plus `tool/call` and `tool/result` in tool-using sessions. |
| Are records contiguous? | Yes in every observed session: `seq` starts at 0 and increases by one per record. The adapter still verifies this instead of assuming it. |
| What is the record envelope? | `{"seq", "type", "time", "data"}` for every event; message-producing events add `surfaceOp`, and `tool/result` adds `sourceEventSeqs`. |

## The bridge

`bridge/dsh-capture/index.mjs` is an out-of-tree Cordis plugin:

- `agent/created` is a serial listener, so the per-agent observer exists before
  the queued prompt can open a turn.
- On each `turn/end` of a non-forked, non-subagent session it spawns
  `coding-memory dsh enqueue` with a bounded hint. This is a hint only: the
  engine re-reads the session and decides.
- On agent disposal it reads the session through `ctx.sessionQuery.readSession`,
  writes an atomically published projection, and runs `coding-memory dsh capture`.
- It projects only replay-validated logical events, so the engine never parses
  the zstd JSONL files, follows format migrations, or pins a storage layout. The
  plugin is dependency-free.

Two failure modes were found and fixed during this experiment:

- A bare `import '@deepseek-ai/schemastery'` from an out-of-tree plugin does not
  resolve in the workspace, and a plain object `Config` makes the loader throw
  on `Config["~standard"].validate`. The bridge now declares its schema in the
  standard-schema shape itself.
- `session/disposed` does not fire for the headless runner's session: the runner
  disposes the tree without detaching the store entry. `agent/disposed` is the
  reliable lifecycle edge, so the per-agent observer and its capture step are
  owned by an `agent.ctx.effect` that runs on that edge.

## Engine-side contract

The adapter (`src/dsh.rs`) applies these rules deterministically; they are
covered by `tests/dsh.rs`:

- Supported logical format is `version <= 4`. A newer format is held as
  `unsupported_history_version` because field meaning can change.
- Excluded: seeded (forked) sessions, `origin: "subagent"`, `delegationDepth > 0`,
  turns with no human `user/message`, and turns that ended at or before project
  registration.
- Injected user-role messages are dropped from evidence; they are harness framing,
  not the human's run.
- A type outside the bridge's known vocabulary, or any type the bridge chooses
  not to project, is held as `unknown_session_event_type`; the turn is
  inspectable instead of silently narrowed. Every type observed in this
  experiment is now either evidence or bookkeeping.
- Outcomes map `completed → completed`, `error → failed`,
  `aborted`/`interrupted → interrupted`; `blocked`, `max-tokens`, `forked`, and
  plugin-defined kinds are held as `unsupported_turn_end_reason` instead of
  guessed.
- Item identities come from the event payload (`message.id`, `toolCallId`,
  `callId`). A collision is `duplicate_item_identity`. Tool results intentionally
  share their call's identity, as `sourceEventSeqs` references it.
- Project attribution uses the session header `cwd`; a projection whose recorded
  session directory no longer resolves to the registered project is held as
  `project_mismatch`.
- Evidence reuses the existing envelope and `items`/`coverage` shape, so
  extraction, consolidation, review, and MCP retrieval need no DSH branch.

## Compaction measurement

A compaction run drives the real `/compact` command after the first turn
(`experiments/dsh_capture/compaction_probe.mjs`, experiment-only, mounted only by
the experiment). Measured on 2026-10-20 (Asia/Tokyo), Ollama Cloud
`ollama-cloud` / `deepseek-v4.1-flash:1017`.

### What compaction records

A successful compaction (`Compacted 4 history items (~1595 tokens)`) appends:

| seq | type | note |
| --- | --- | --- |
| 19 | `command/run` | `/compact` with no turn and no prompt content |
| 20 | `compaction/start` | `turn: null` |
| 22 | `compaction/summary` | the summary blocks plus `shadowedSeqs` and `shadowedTokenCount` |
| 23 | `user/message` | **`surfaceOp: {op: "replace", startSeq: 8, endSeq: 11}`**, the condensation checkpoint |
| 24 | `compaction/end` | no error |
| 25 | `command/done` | success text |

Two consequences the adapter must respect:

- The bracket and the command pair carry **no turn attribution**. They are
  harness bookkeeping; an engine that requires every in-turn record to name its
  turn holds the whole run.
- Compaction does **not** rewrite the completed turn's evidence. The human
  messages, the assistant answer, and `tool` records keep their original seqs;
  the replacement only changes the model's current surface. A captured turn
  therefore stays a faithful record of what was asked and answered, before
  condensation.

### Captured after a successful compaction

The persisted log with a successful compaction was projected (the same
projection shape the bridge writes) and captured: the run reached `ready`, its
evidence held exactly the human message and the assistant answer, and no
compaction record reached evidence. The same holds for the session capture that
reads the log earlier in the process.

### Why the first compaction experiments failed

- The experiment's `/compact` invocation needs an idle agent; invoking from
  `setImmediate` raced the driver (`Compaction cancelled.`). The first bracketed
  failure surfaced a stale `error: no adapter registered for provider` from that
  race; it was never a route defect. A direct probe showed `ollama-cloud`
  registered from plugin config alone, and compaction now succeeds 3/3 both with
  and without a pinned summarization route.
- For unattended use, pin `summarizationProvider`/`summarizationModel` in the
  composition anyway: the summarizer otherwise falls back to the latest routed
  request, which an experiment (or a configuration change) can leave unset.

### Capture ordering around compaction

A turn can end seconds before a `/compact` of the same session finishes, so a
projection read at `turn/end` can be pre-compaction. The bridge:

- waits for an observed `compaction/start` to close (bounded by `timeoutMs`), and
- waits for a producer's announced intent, emitted synchronously at `turn/end`
  (`agent.ctx.emit('coding-memory/compaction-requested', { sessionId })`).

The wait is costless when no compaction is announced, because a pending timer
would keep the process alive past the teardown point where the read must already
be in flight. With the wait, a capture reads the finished bracket
(`compaction/start`, `compaction/summary`, `compaction/end`) and still publishes
only the turn's own message and answer.

Measured matrix (one run each, same route): plain run, compacted run, resumed
run, and resumed-then-compacted run all reached `ready`; the resumed cases
captured turn 2 only.

## Resume measurement

A resume run continues the first session in a second process
(`dsh --profile headless --session-id <id>`, experiment `--resume`). Measured on
2026-10-20 (Asia/Tokyo), same route.

| Observed | Result |
| --- | --- |
| Header after resume | `isSeeded: false`, unchanged id; the process appends `session/end-seed` |
| Turn numbering | continues (`turn/start {turn: 2}`); turn 1 keeps its records |
| Captured | turn 2 only, `ready`, evidence = the new human message and its answer |
| Turn 1 re-captured | no (already `ready`; the queue ignores duplicates) |
| Compaction issued in the resumed process | bracket appended after `session/end-seed`; turn 2 still captured with its own evidence |

`session/end-seed` is therefore a **resume marker**, not proof of a fork: an
untouched header (`isSeeded: false`) gets one on `agents.resume`. Treating it as a
fork boundary would silently drop every turn of every resumed session; the
adapter excludes real forks by the header instead, and a regression test now
covers a resumed session.

## Defects found while measuring compaction

These were found by the measurement and are fixed with regression tests:

1. **Cross-run capture.** The bridge captured on `session/disposed`, which fires
   when a *later* process reads that session, not when this run ends. Capture is
   now anchored on `turn/end`, and the engine refuses to publish a run without a
   hint recorded by a live runtime (`no_capture_hint`).
2. **Pre-registration hints were lost.** Hints are now persisted and resolved by
   `reconcile` to their exact turn; a run that ended before enablement is
   excluded exactly like a baseline turn.
3. **`reconcile` queued runs nobody observed ending.** It now only resolves hints.
4. **Projected logs stopped replaying.** Stripping the provider route from
   `assistant/message` sources makes DSH's own replay fail
   (`message must have model source`), and flattening a user message fails with
   `not losslessly JSON-serializable`. The bridge projects envelope fields and
   leaves `data` intact, proven by replaying real and projected logs through
   `Session.create`.
5. **A message's turn is not a field.** `user/message` carries no `turn`; it
   belongs to the enclosing `turn/start`. Attribute-scanning made every real
   turn look non-human (`history_missing`).
6. **Bridge reads failed under the runner's teardown.** A read started after the
   engine subprocess finished resolves against an inactive fiber. The read now
   starts one microtask after the commit, before the hint runs.

## Limitations and deferrals

- Fork and subagent capture are excluded. A fork's turn prefix is inherited
  context; capturing it without that context would produce a misleading run.
- Compaction replaces earlier surface nodes with a `replace` operation. Capture
  keeps the raw log window of the turn, so a compacted turn is still recorded
  faithfully; a later turn that builds on the summary carries the checkpoint
  message as its own evidence, and the replaced span stays out of it.
- Capture waits for a compaction that starts within the settle window of the
  turn's end. A `/compact` issued minutes later is outside that window; its
  bracket is then simply appended after the captured turn, which stays a
  faithful record of what was asked and answered.
- Attachment bytes, provider-hidden reasoning, and any event the bridge drops are
  outside evidence and listed in its `coverage`.
- A resumed session's earlier turns predate the registration epoch only when
  they were created before it; turns created after registration keep being
  captured across resumes, as measured.
- Only one provider route was exercised. Nothing in the adapter depends on the
  route, but a route that changes the event vocabulary would surface as a held
  turn, not silent capture.

## Reproduction

```sh
nix develop --command cargo build

# Pipeline only, no model call.
python3 experiments/dsh_capture/run.py \
  --dsh /path/to/deepseek-harness/apps/cli/lib/bin.js \
  --engine "$PWD/target/debug/coding-memory"

# Credentialed: mounts a credential document into the isolated home, which is
# removed with the rest of the synthetic root.
python3 experiments/dsh_capture/run.py \
  --dsh /path/to/deepseek-harness/apps/cli/lib/bin.js \
  --engine "$PWD/target/debug/coding-memory" \
  --credentials /path/to/isolated-credentials.yaml \
  --provider ollama-cloud --model deepseek-v4.1-flash:1017 \
  --task "Say hi in one short sentence."

# Compaction after the first turn (experiment-only `/compact` driver), with the
# summarization route pinned in the composition as unattended use requires.
# Add --compact-after-turn 1 --pin-summary to the credentialed command above.

# Resume: the second process continues the first session.
# Add --resume (optionally with --compact-after-turn 1).
```
