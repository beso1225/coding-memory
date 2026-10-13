# Codex Capture Experiment

Date: 2026-10-13 (Asia/Tokyo). Work item: [issue #1](https://github.com/beso1225/coding-memory/issues/1).

Status: the capture feasibility checkpoint is validated for the installed CLI
and local ChatGPT App. The user approved project-local hooks in the App GUI and
repeated the synthetic turn; a separate bundled app-server read that App-created
thread without resuming it. This validates the selected interfaces, not a finished
adapter or unattended operation.

## Environment and method

| Surface | Installed version | Execution tested |
| --- | --- | --- |
| PATH Codex | `0.162.1` | `codex exec` and its stdio app-server |
| ChatGPT App | `26.1007.30912`, build `20156` | Human-run synthetic turn, GUI hook approval, independent cold read |
| App-bundled Codex | `0.162.0-alpha.17.2` | Separate exec and stdio app-server probes |

Source revisions corresponding to the installed executables:

- CLI tag `rust-v0.162.1`: [`092d3acd`](https://github.com/openai/codex/tree/092d3acd6bec3e3a14bdc7e7a2810ab628ab759d).
- Bundled tag `rust-v0.162.0-alpha.17.2`: [`740e5af3`](https://github.com/openai/codex/tree/740e5af33c71225640e0c1c1555c514c2c93ab74).

The [probe](../experiments/codex_capture/README.md) uses a fresh synthetic project,
isolated runtime state, a loopback Responses provider, and generated recording
hooks. It exercises the installed binaries without model-account authentication
or reading personal transcript bodies. Hook trust is bypassed only for those
generated scripts in the isolated probe. The later App GUI check used normal
hook approval, without bypass. The interactive CLI TUI was not exercised.

The fixtures are small projections of the observed API/history records. Generated
runtime instruction bodies and machine-specific paths are excluded. No upstream
implementation code was copied into the project. The fixture assertions validate
captured contracts; they do not execute the runtime again or measure LLM quality.

## Observed lifecycle and identifiers

These behaviors were independently reproduced with both executables:

| Case | Observation | Capture implication |
| --- | --- | --- |
| Exec completes | JSON output contains `thread.started` with `thread_id`; `turn.completed` has usage but no native turn ID | Exec JSON alone cannot identify a native turn reliably |
| Legacy notify | `agent-turn-complete` carries `thread-id`, `turn-id`, `cwd`, input messages, and final answer | Use the two IDs as a hint; do not use the answer as the evidence bundle |
| Hook trust | Untrusted generated lifecycle hooks did not execute; explicit bypass made them execute | Installing a hook file alone does not establish capture readiness |
| Stop continuation | Two Stop invocations (`stop_hook_active` false, then true), two assistant messages, one native turn, one terminal outcome | Stop invocation is not the final run boundary |
| Tool step | `commandExecution` retains command, result text, exit code, and native item identity in the selected turn | Preserve relevant tool evidence and its item locator |
| Steering | `turn/steer` returns the active native turn ID; two user messages belong to that turn | One user message is not one Run |
| Provider failure | Synthetic HTTP 400 produces `failed`, with error details in stored history | Confirm failure from persisted status |
| SIGINT / API interruption | Exec SIGINT and `turn/interrupt` produce stored `interrupted` | Record interruption independently from failure |
| Exec exit code | Both synthetic failure and SIGINT exit with code 1 | Process exit code alone cannot distinguish those outcomes |
| Failure/interruption notify | No successful legacy notification was observed for these cases | Reconciliation is necessary to collect terminal outcomes without a success notification |

`turn/completed` notifications from app-server include a native turn ID and final
status, but their item view can be a summary or not loaded. Retrieve full history
for evidence rather than treating terminal notification items as complete.

The two versions did not render steering identically: the exploratory bundled
probe retained the pre-steer assistant message while the CLI probe omitted it.
Both preserved one turn and both user instructions. The contract preserves the
items actually returned; it does not require identical intermediate messages.

## Notification and persistence ordering

At each Stop hook during the continuation experiment, the readable rollout tail
contained the response/token records but not the final `task_complete` marker.
This is a direct example of a hook running before terminal persistence.

In the observed legacy-notify callbacks, `task_complete` was already readable.
That does not establish an ordering guarantee: command startup can occur after
the runtime has continued. No fixed readiness-delay bound was measured.

Both tagged source versions place legacy notification before the final task
completion path. The notification implementation spawns its command without
waiting for durable enqueue acknowledgment. The completion path sends the
terminal event and then flushes the rollout. Thus neither notification delivery
nor an app-server terminal event replaces a stored-history readiness check.

Sources: CLI [turn lifecycle](https://github.com/openai/codex/blob/092d3acd6bec3e3a14bdc7e7a2810ab628ab759d/codex-rs/core/src/session/turn.rs#L711),
[notification spawning](https://github.com/openai/codex/blob/092d3acd6bec3e3a14bdc7e7a2810ab628ab759d/codex-rs/hooks/src/legacy_notify.rs#L80),
[completion flush](https://github.com/openai/codex/blob/092d3acd6bec3e3a14bdc7e7a2810ab628ab759d/codex-rs/core/src/tasks/mod.rs#L881),
and independently inspected bundled [turn lifecycle](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/core/src/session/turn.rs#L711)
and [completion flush](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/core/src/tasks/mod.rs#L881).

The proposed worker waits/retries until the selected native turn has a terminal
status, persisted completion metadata, and a full item view. Pending history,
unsupported history, missing history,
and a failed Run are separate states. Retry timing remains configurable and must
be evaluated with real sessions; the synthetic probe does not justify a latency SLA.

## Selected history route and reader contract

Use a matching Codex app-server as the leading history-reader route, rather than
reimplementing the changing on-disk format in Rust. The following were tested:

- `thread/read(includeTurns=true)` cold-reads persisted threads in a fresh process
  without resuming them. It returns `notLoaded` thread state and full persisted
  turn items in the normal synthetic case.
- Experimental `thread/turns/list` pages newest-first, with an opaque continuation
  cursor. A one-turn page and the next page distinguish the later and earlier turns.
- Experimental `thread/items/list(threadId, turnId)` returns the selected earlier
  turn's tool evidence without the later turn's messages.

Prefer paged turn discovery and turn-filtered item reads for bounded resource
use, gated by the tested runtime version/capability. `thread/read` remains a
demonstrated compatibility route, but an unbounded whole-thread response should
not silently bypass evidence/output limits. The official
[app-server reference](https://learn.chatgpt.com/docs/app-server) marks the paged
methods experimental; their current behavior is not a permanent format guarantee.

The Issue #3 reader should satisfy this contract:

1. Accept a state-store identity, known native thread/turn IDs, project association,
   and supported runtime version. Runtime version is metadata, not a Run identity.
2. Retrieve only the selected turn and require a terminal status plus persisted
   completion metadata (`completedAt` in the tested versions). A status inferred
   for an open history is provisional. Treat missing terminal metadata as pending
   or explicitly inferred crash interruption, rather than confirmed termination.
   Do not infer
   completion from a hook, the last assistant message, process exit, or thread idle.
3. Require complete pagination of the selected item view within configured limits.
   A partial/summary view cannot silently become successful evidence capture.
4. Preserve native item IDs, user instructions, assistant messages, applicable
   tool results, and recorded truncation/coverage information. Exclude unrelated
   turns even if the source response contains them.
5. Capture the result immutably before extraction. Identify a source/API projection
   separately from any retained original artifact; do not call projected API items
   a complete raw transcript.
6. Return pending, unsupported, missing, or over-limit outcomes explicitly. Never
   resume a development thread as a side effect of reading evidence.

The offline fixture test selects the earlier native ID from a response obtained
after a later turn. It asserts the earlier tool result remains present and the
later user instruction/answer are absent. Pagination fixtures demonstrate a route
that can obtain that evidence without loading the later items.

## Resume, compaction, and inherited history

Resume preserved the thread identity and old turn IDs; the new turn received a
new ID. A cold read before resume did not load the thread into an active session.

Explicit compaction produced a separate completed `contextCompaction` turn and
kept the preceding user turns available through history APIs. Reading at item
completion can still observe a provisional compaction turn. In the bundled probe,
one such read returned `interrupted` with null `completedAt`, while the terminal
event and later read returned `completed` with a completion timestamp. The source
normalizes open turns to interrupted when its reader does not see a live running
thread. Thus a terminal-looking status is insufficient for an independent observer;
check persisted completion metadata. This synthetic explicit-compaction result does not establish
every automatic-compaction or context-reset behavior. A compaction-only turn has
no new human instruction and should not be independently extracted.

Forking through the first turn produced a child thread with the parent's first
native turn ID followed by a new child turn ID. The child file contained a
`history_base` reference instead of a standalone copy of the whole inherited
history. Its `forked_from_ordinal_exclusive` matched the inherited prefix boundary
and the file's first ordinal began at that boundary.

Consequences:

- A raw child-file-only reader can miss inherited context. Let the matching
  runtime resolve the logical history, and record inherited context as coverage.
- Inherited turns are context, not new child work. Skip inherited prefix turns
  before enqueueing child-owned Runs; deduplication by child thread ID alone is
  insufficient.
- Keep the source history owner and prefix boundary for provenance. Byte offsets
  and ordinals belong to the original rollout, not the exported fixture.
- Shared-prefix deletion, archive, ephemeral sessions, remote App threads, abrupt
  power loss, and future format versions were not tested by this probe.

## Reconciliation proposal

This is an implementation contract proposal, not an implemented reconciler:

- Register projects and runtime state stores explicitly. CLI and App observers
  sharing one Codex state store must share its namespace; different observers or
  runtime upgrades must not turn the same native Run into duplicate work.
- Record an enablement epoch and a baseline of existing native turns, including
  active turns. Default collection begins with new human-requested turns after
  enablement; older/in-progress baseline work requires deliberate backfill.
- Scope discovery to registered canonical repositories and their worktree/root
  mappings. Resolve each turn's project association rather than trusting only a
  thread's latest `cwd`; reject unregistered paths and ambiguous associations.
- Persist per-thread known own-turn boundaries and accepted native IDs, plus the
  enablement epoch. Treat API page cursors as temporary traversal state, not a
  guaranteed durable watermark.
- Revisit active/pending threads, overlap recent discovery, and periodically scan
  the registered scope. Thread update times help discovery but cannot be the
  only correctness cursor. A duplicate live/backfilled signal maps to the same
  `(state_store_id, history_owner_thread_id, native_turn_id)` unique key.
- Exclude inherited prefixes, compaction-only/child work, and an explicit registry
  of engine-owned generation runs. `source`, `originator`, and ephemeral status
  alone do not establish human ownership or prevent recursive capture.
- Advance durable processing state only after evidence has been captured. A lost
  notification can be repaired from stored history; never-persisted or deleted
  history cannot be promised recoverable.

## App feasibility checkpoint

The actual ChatGPT App backend was observed running the nested bundled
`CodexCLI.app` executable with `app-server --analytics-default-enabled`; no
external `--listen` endpoint was present in the inspected invocation. It had
`~/.codex/state_5.sqlite`, `thread_history_1.sqlite`, and a session rollout open.
Only process/file-location metadata was inspected, not personal transcript bodies.

The isolated bundled-binary probe independently demonstrated executable hooks,
legacy notify, stored history, fork-prefix resolution, and cold API reads. The
subsequent human-run GUI probe established the actual local App route:

1. The first literal `APP_CAPTURE_OK` turn completed and was readable by a separate
   bundled app-server, but no recording-hook log existed yet. Project trust was
   present; per-hook approvals were not present at inspection time.
2. The user then saw the App's hook approval UI, approved the generated hooks, and
   repeated the instruction in the same App conversation. Saved trust hashes
   were present for all five generated hook handlers.
3. `UserPromptSubmit` and `Stop` were recorded with the App thread ID, native turn
   ID, working directory, and transcript locator. The recorder stores metadata,
   not conversation text. SessionStart was not expected to repeat for this
   already-created conversation; tool/interruption hooks were not exercised here.
4. A fresh matching bundled app-server read the same thread via `thread/read` and
   `thread/items/list`. Both synthetic turns were completed, the later turn's
   ID matched both recorded hooks, and its full items contained `APP_CAPTURE_OK`.
   The thread was `notLoaded`; no `thread/started` notification was observed.

The [App GUI fixture](../experiments/codex_capture/fixtures/app-gui-26.1007.30912.json)
and its offline assertion preserve this result with machine paths anonymized.
The [cold-reader probe](../experiments/codex_capture/read_app_thread.py) only uses
initialization and read methods against explicitly supplied synthetic thread IDs.
Hook/notify disabling in that separate reader is invocation-local and does not
change the App's settings or resume the development thread.

No legacy `agent-turn-complete` notification appeared in the GUI probe's log,
despite the prepared project-local notify configuration. Its cause was not
isolated; do not generalize absence in this configuration to every App version.
Select the demonstrated project-local Stop hook as the App capture hint, with
native history confirmation and reconciliation. CLI legacy notify remains a
demonstrated hint for the CLI. Repeated Stop invocations share the native Run key;
none alone confirms termination. Use Interrupt as an additional hint only after
validating its actual App behavior, and reconcile persisted terminal outcomes.

The available Computer Use tool rejected GUI control of this App
(`com.openai.codex`) for safety reasons. No alternate GUI automation route was used.
The user performed the GUI step manually and approved the synthetic project's
hooks. The collector was not installed for normal projects.

The [GUI procedure](../experiments/codex_capture/README.md#chatgpt-app-gui-check)
can repeat this check for another installed version. The Issue #1 feasibility
checkpoint is satisfied by the demonstrated hook and cross-process history route.
App restart, App-owned tool/interruption turns, GUI approval timing, large history,
and remote/cloud App threads remain unmeasured. Validate those scenarios when
implementing App daily-use support; do not claim live subscription to the existing
App-owned event stream or reliability through restart from this experiment.
