# Runtime and OSS Research

Research date: 2026-10-13 (Asia/Tokyo).

This file records the original design investigation. Subsequent installed-binary
experiments and their remaining App checkpoint are recorded in
[codex-capture-experiment.md](codex-capture-experiment.md).

This record distinguishes documentation/source inspection from runtime experiments.
No capture plugin, engine, model extraction, or live integration experiment was
implemented or executed during design research. Installed version and help output
were inspected; personal transcript bodies and authentication material were not.

Upstream behavior can change. Preserve the references below as the evidence for
this design baseline and repeat relevant checks when implementing an adapter.

## Local Codex versions

The user updated the CLI and ChatGPT App during the design discussion. The later
local inspection found:

| Component | Inspected version |
| --- | --- |
| PATH Codex CLI | `0.162.1` |
| `/Applications/ChatGPT.app` | `26.1007.30912`, build `20156` |
| App-bundled Codex | `0.162.0-alpha.17.2` |

Earlier findings used CLI `0.159.2`, App `26.1002.52244` / build `13536`, and
bundled Codex `0.162.0-alpha.2`. Those are historical observations, not the
current validation target. Updating both products did not make their Codex
versions identical.

Two public source revisions were inspected:

- General lifecycle investigation: [`b043762f`](https://github.com/openai/codex/tree/b043762f9e3eb65dd5164ff5880f139bc6d084c7),
  upstream main at inspection time.
- CLI `0.162.1` notification registration follow-up:
  [`092d3acd`](https://github.com/openai/codex/tree/092d3acd6bec3e3a14bdc7e7a2810ab628ab759d).

Lifecycle findings from main have not been demonstrated in either installed
distribution. The updated App's bundled source behavior has not been fully
compared with the CLI tag.

## Codex capture and history

Official [advanced configuration](https://learn.chatgpt.com/docs/config-file/config-advanced)
documents legacy `notify` for `agent-turn-complete`, carrying identifiers and
completion-related context. Official [Hooks documentation](https://learn.chatgpt.com/docs/hooks)
describes lifecycle hooks, trust review, and a `transcript_path` whose format is
not a stable history API.

The inspected main source establishes several limits:

- `Stop` can request continuation of the same turn. It is not necessarily a
  terminal event for a human-requested run.
- Legacy notification is dispatched before post-turn work and final completion.
  A notification is not proof that final history is already readable.
- Legacy notification returns after spawning its command, without waiting for
  command completion or acknowledging successful queue registration.

Sources: [turn lifecycle](https://github.com/openai/codex/blob/b043762f9e3eb65dd5164ff5880f139bc6d084c7/codex-rs/core/src/session/turn.rs#L660),
[legacy notification](https://github.com/openai/codex/blob/b043762f9e3eb65dd5164ff5880f139bc6d084c7/codex-rs/hooks/src/legacy_notify.rs),
[task completion](https://github.com/openai/codex/blob/b043762f9e3eb65dd5164ff5880f139bc6d084c7/codex-rs/core/src/tasks/mod.rs),
and [hook runtime](https://github.com/openai/codex/blob/b043762f9e3eb65dd5164ff5880f139bc6d084c7/codex-rs/core/src/hook_runtime.rs).

The official [App Server API](https://learn.chatgpt.com/docs/app-server) offers
`thread/read(includeTurns=true)` without resuming the thread, and terminal status
through `turn/completed`. Steering does not necessarily create a new turn.
These are useful integration surfaces, but starting an engine-owned app-server
is different from subscribing to a backend already owned by ChatGPT App.

**Not established:** an independent observer can subscribe to App-owned live
events; an engine-owned app-server can read every App history form without
unwanted side effects; updated CLI and App execute identical hooks. Do not infer
these properties from a shared configuration directory or an API name.

### Capture experiment requirements

Use small synthetic sessions and inspect CLI and App separately:

1. Observe normal completion, multiple tool steps, steering, Stop continuation,
   failure, interruption, and application shutdown.
2. Record notification identifiers, hook trust/configuration behavior, executable
   environment, and the delay until final history is readable.
3. Retrieve one completed turn without including a later turn. Check resume,
   compaction, archival, and fork/inherited history.
4. Assess the supported history API first. If a file decoder is required, pin its
   runtime/version fixtures; do not treat `history.jsonl` as a complete transcript.
5. Simulate missed and duplicated registration, then reconcile only registered
   projects from their enablement boundary.
6. Establish a viable App route before relying on it for the daily-use milestone.
   If the route is absent, report the constraint and revisit the adapter design.

## Codex as the generation backend

Local CLI `0.162.1` help confirmed `--output-schema`, `--json`,
`--output-last-message`, `--ephemeral`, `--ignore-user-config`, model/config
overrides, and `--thread-source`. The meaning and accepted values of a custom
thread source have not been established as a collector-exclusion contract.

Official [non-interactive documentation](https://learn.chatgpt.com/docs/non-interactive-mode)
describes `codex exec`, structured final output, and reuse of saved CLI
authentication. A fresh exec session can serve the extraction, consolidation, or
review role independently of the development session's model.

Isolation requires an experiment:

- `read-only` does not prohibit all tool use.
- `--ephemeral` does not disable hooks.
- Help did not expose a single flag guaranteeing that every tool is disabled.
- `--ignore-user-config` has not been proven to remove every project, plugin, or
  managed configuration layer.
- In the inspected CLI tag, legacy notification registration is separate from
  the lifecycle-hook feature gate. Disabling lifecycle hooks alone cannot be
  assumed to disable legacy `notify`.

Sources: [configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference),
[Hooks](https://learn.chatgpt.com/docs/hooks), and
[CLI-tag notification registration](https://github.com/openai/codex/blob/092d3acd6bec3e3a14bdc7e7a2810ab628ab759d/codex-rs/hooks/src/registry.rs#L124).

Validate explicit notification/hook settings, minimized tools and MCP access,
collector exclusion of engine-owned runs, JSON Schema results, exit handling,
timeouts, and output limits. Preserve a fixture demonstrating that generation
does not recursively enqueue itself, including during later history reconciliation.
No claims about performance, quota consumption, or model quality have been
measured yet.

## DeepSeek Harness

Inspected fork: [`975e4aff`](https://github.com/shiguredo/deepseek-harness/tree/975e4aff0c6a3c90af42005eef557b8720dd0a00),
commit dated 2026-10-12 JST. Source inspection only; no plugin was run.

### Lifecycle

A Cordis host plugin can observe `session/event` and select `turn/end`.
`agent/turn-stopping` is an awaited pre-close hook that can steer further work;
it is not the final notification boundary. Built-in end reasons include
`completed`, `aborted`, `blocked`, `error`, `max-tokens`, `interrupted`, and
`forked`, and plugin-defined reasons are possible.

Sources: [agent loop](https://github.com/shiguredo/deepseek-harness/blob/975e4aff0c6a3c90af42005eef557b8720dd0a00/packages/core/agent-loop/src/agent.ts),
[turn/event types](https://github.com/shiguredo/deepseek-harness/blob/975e4aff0c6a3c90af42005eef557b8720dd0a00/packages/core/session/src/types.ts),
and [runtime hook contract](https://github.com/shiguredo/deepseek-harness/blob/975e4aff0c6a3c90af42005eef557b8720dd0a00/packages/core/agent/src/runtime-types.ts).

`session/event` is a post-commit, fire-and-forget notification. Listener errors
are isolated, but the plugin shares the host process and can still delay it with
synchronous work. `turn/end` does not itself await disk flush. An async plugin
callback can also be lost when the host crashes before enqueue completes.

Sources: [event notification](https://github.com/shiguredo/deepseek-harness/blob/975e4aff0c6a3c90af42005eef557b8720dd0a00/packages/core/session/src/index.ts),
[checkpoint policy](https://github.com/shiguredo/deepseek-harness/blob/975e4aff0c6a3c90af42005eef557b8720dd0a00/packages/session/session-checkpoint-policy/src/index.ts).

### History and bridge

Session ID, event sequence/time, turn/step, source of user messages, and inherited
event count support bounded capture and deduplication. Save the relevant end
sequence when observing completion; reading the entire later session would admit
subsequent turns.

`ctx.sessionQuery.readSession(id)` provides validated logical events, with live
session support and cold-read handling. The inspected logical session format is
V4 and default JSONL persistence is zstd-compressed. Paths depend on the runtime
instance. A plain, unversioned JSONL reader is not a sound assumption.

Sources: [session query](https://github.com/shiguredo/deepseek-harness/blob/975e4aff0c6a3c90af42005eef557b8720dd0a00/packages/session-query/session-query/README.md),
[format version](https://github.com/shiguredo/deepseek-harness/blob/975e4aff0c6a3c90af42005eef557b8720dd0a00/packages/core/session/src/types.ts#L89),
and [persistence provider](https://github.com/shiguredo/deepseek-harness/blob/975e4aff0c6a3c90af42005eef557b8720dd0a00/packages/session/session-persistence-jsonl/README.md).

A thin TypeScript bridge to logical history is the leading future option, compared
with implementing compression, migration, and versioned decoding in Rust. This
is a proposal, not a completed bridge. Evaluate flush barriers, retry/reconciliation,
fork prefixes, synthetic interrupted endings, and root-versus-child capture.

Tool-call/result events provide recorded arguments, model-visible results, and
metadata, but not a blanket guarantee of complete tool stdout or attachments.
The Web ZIP export is browser-oriented, not the intended host export contract.

DSH's Ollama Cloud/local routing belongs to its development runtime and need not
control Coding Memory's independently selected generation backend.
[Provider implementation](https://github.com/shiguredo/deepseek-harness/blob/975e4aff0c6a3c90af42005eef557b8720dd0a00/packages/llm/llm-pi-ai/README.md).

## Memory-system references

### agent-memory

Inspected [`3698e601`](https://github.com/tigerless-labs/agent-memory/tree/3698e601e07665121b9388aca100364837518e20),
commit dated 2026-10-13 JST.

- Markdown is authoritative; SQLite provides search/projection data. Watermarks,
  archives, and pending operations exist separately, so not all processing state
  is disposable.
- Hook capture archives a normalized transcript delta and starts detached
  distillation. This is not the proposed durable SQLite queue and candidate-first
  worker pipeline.
- Distillation can directly create/update/supersede memory, with bounded target
  handles and validation. Management is a separate process.
- Hash-based precondition checks, atomic file replacement, validity periods,
  supersession, and provenance are useful references. These do not establish
  whole-batch filesystem/SQLite crash atomicity.
- Related links use access co-occurrence and frontmatter relationships, rather
  than the proposed semantic Wikilink review.

Sources: [capture](https://github.com/tigerless-labs/agent-memory/blob/3698e601e07665121b9388aca100364837518e20/packages/adapters/src/agent_memory/adapters/capture.py),
[distillation](https://github.com/tigerless-labs/agent-memory/blob/3698e601e07665121b9388aca100364837518e20/packages/core/src/agent_memory/core/distill.py),
[management](https://github.com/tigerless-labs/agent-memory/blob/3698e601e07665121b9388aca100364837518e20/packages/core/src/agent_memory/core/manage.py),
and [store](https://github.com/tigerless-labs/agent-memory/blob/3698e601e07665121b9388aca100364837518e20/packages/core/src/agent_memory/core/store.py).

Its Codex parser mainly selects user/assistant message records. Its archived
filtered transcript should not be mistaken for a byte-complete rollout with tool
results. Query normalization also uses ASCII word patterns, so Japanese-only
queries can become empty.
[Transcript parser](https://github.com/tigerless-labs/agent-memory/blob/3698e601e07665121b9388aca100364837518e20/packages/adapters/src/agent_memory/adapters/transcript.py),
[search implementation](https://github.com/tigerless-labs/agent-memory/blob/3698e601e07665121b9388aca100364837518e20/packages/core/src/agent_memory/core/search_index.py).

### Anamnesis

Inspected [`9d894360`](https://github.com/Trapezohe/Anamnesis/tree/9d894360f96ea05ff30205c08c8d07ba9725f7f9),
commit dated 2026-06-02 JST.

- The main pipeline imports existing memories through scan/watch/normalize
  adapters into normalized SQLite records, chunks, raw artifacts, and provenance.
  It is not an authoritative Markdown-vault writer.
- Adapter contracts, stable source identities, provenance, and isolation of
  individual import failures are useful references.
- LLM extraction and provider abstractions exist. Some comments describe an
  older incomplete stage; inspect the actual stage implementation.
- Hybrid retrieval, Chinese-oriented segmentation, and approximate duplicate
  detection do not establish Japanese retrieval quality or semantic consolidation.

Sources: [adapter contract](https://github.com/Trapezohe/Anamnesis/blob/9d894360f96ea05ff30205c08c8d07ba9725f7f9/crates/core/src/adapter.rs),
[domain records](https://github.com/Trapezohe/Anamnesis/blob/9d894360f96ea05ff30205c08c8d07ba9725f7f9/crates/core/src/model.rs),
[importer](https://github.com/Trapezohe/Anamnesis/blob/9d894360f96ea05ff30205c08c8d07ba9725f7f9/crates/importer/src/lib.rs),
[extraction](https://github.com/Trapezohe/Anamnesis/blob/9d894360f96ea05ff30205c08c8d07ba9725f7f9/crates/extractor/src/stage2.rs),
and [Codex parser](https://github.com/Trapezohe/Anamnesis/blob/9d894360f96ea05ff30205c08c8d07ba9725f7f9/crates/adapter-codex/src/session.rs).

### Licensing

| Reference | Inspected license | Reuse consideration |
| --- | --- | --- |
| agent-memory | [MIT](https://github.com/tigerless-labs/agent-memory/blob/3698e601e07665121b9388aca100364837518e20/LICENSE) | Preserve required notices if code is copied |
| Anamnesis | [Apache-2.0](https://github.com/Trapezohe/Anamnesis/blob/9d894360f96ea05ff30205c08c8d07ba9725f7f9/LICENSE), with [NOTICE](https://github.com/Trapezohe/Anamnesis/blob/9d894360f96ea05ff30205c08c8d07ba9725f7f9/NOTICE) | Check notice and modified-file obligations for actual reuse |
| DSH | [MIT](https://github.com/shiguredo/deepseek-harness/blob/975e4aff0c6a3c90af42005eef557b8720dd0a00/LICENSE) | Check dependency notices separately if reusing dependencies or bundled code |

The current documents use these projects as design references. No upstream code
has been copied. Review the exact files and dependency licenses before any future
code reuse; the repository-level license is not a license inventory of every file.
These documents do not select Coding Memory's own distribution license.

## Retrieval and provider observations

[SQLite FTS5 documentation](https://www.sqlite.org/fts5.html) distinguishes word
tokenization and trigram substring matching. A small in-memory experiment using
local SQLite `3.51.0` searched the sentence
`SQLiteの永続キューで再試行を実装する`:

| Query | `unicode61` | `trigram` |
| --- | --- | --- |
| `永続キュー` | No match | Match |
| `実装` | Not tested | No match |

This is a tokenizer check, not a retrieval-quality benchmark. Trigram full-text
queries do not match substrings shorter than three characters. The agreed MVP
instead uses English memory bodies and agent-formed English/identifier queries;
Japanese retrieval remains a future evaluation topic.

[Ollama structured-output documentation](https://docs.ollama.com/capabilities/structured-outputs)
currently states that Cloud does not support structured outputs. Native
[chat API](https://docs.ollama.com/api/chat) capability and Cloud capability must
not be assumed identical. Any future backend needs engine-side validation and
clear failure handling even when schema-constrained generation is supported.

For future Pi work, distinguish the
[model API library](https://github.com/earendil-works/pi/blob/main/packages/ai/README.md)
from its coding-agent runtime. Current
[extension documentation](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md)
distinguishes settled execution from an `agent_end` after which automatic work
can continue. Pi was not pinned or executed in this research; re-inspect it when
its integration becomes scheduled work.
