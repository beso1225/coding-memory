# Codex Generation Isolation Experiment

Date: 2026-10-14 (Asia/Tokyo). Work item: #2.

This is an installed CLI 0.162.1 experiment, not the production generation
backend. The three roles use separate exec processes and distinct configurable
model names against the synthetic loopback Responses provider. No personal
history, authentication file, or paid model request was used. This demonstrates
runtime plumbing and output handling, not real LLM knowledge quality or successful
reuse of ChatGPT authentication in a dedicated generation home.

Run `python3 experiments/codex_generation/run.py /opt/homebrew/bin/codex`.
Raw results stay under `/private/tmp/coding-memory-capture-*`. The committed
`observed.json` is a projection with local paths removed. Run offline assertions
with `python3 -m unittest discover -s experiments/codex_generation`.

## Results

Fresh extraction, empty extraction, consolidation, and rejected review produced
locally validated output. The provider observed each role's selected model and
JSON Schema request. The spike schema has only role, string items, and approval;
it is not the Candidate/Change Set schema planned for later domain implementation.
Malformed model output was classified as invalid despite exit zero. Synthetic
HTTP 400 was process failure; a delayed response caused a distinct timeout.
Review rejection never implies permission to publish. Validation has a final
output limit, but subprocess capture is not streaming-bounded yet.

Exact controls: fresh synthetic working directory and CODEX_HOME,
`--ignore-user-config --ignore-rules --sandbox read-only`, explicit `notify=[]`,
hooks and plugins disabled, `--output-schema` and `--output-last-message`.
Generated lifecycle hooks and legacy notify did not produce a log. An MCP process
configured in the ignored user config did not start. This does not establish
that managed requirements or all project configuration are removed.

A deliberate harmless command call still executed. Adding global
`--disable shell_tool --disable unified_exec` did not remove exec_command or
prevent that call in the observed invocation. Other tools also remained exposed.
Do not advertise this configuration as tool-free or a closed-data security
boundary. Output-schema mode is not a tool-disable mechanism. Further tool and
filesystem isolation validation is required before unattended generation over
personal evidence.

## Collector exclusion and policy

The prototype exclusion contract uses a registered, engine-owned state-store
namespace, plus optional owned native thread IDs. It excludes both live and
reconciled events by the same predicate. The probe intentionally persists its
runs instead of relying on ephemeral sessions, and its native thread IDs appear
in saved rollout headers. The predicate tests also cover a missed notification.
This is a deterministic prototype, not the durable registry/collector integration
belonging to #3. Persist namespace ownership before spawning generation and keep
it across restarts and authentication provisioning. Never infer internal work
solely from its last message, source label, model, or ephemeral flag.

The external-provider policy gate runs before the execution callback. A denied
project returns held_external_policy and makes no generation call. Timeout,
process failure, invalid output, empty extraction, and reviewer rejection remain
separate outcomes. The Rust backend must retain these distinctions, use fresh
output paths, validate the full domain schema and evidence locators, terminate
owned children on timeout, and bound stdout/stderr while reading.

## Remaining checkpoint

Issue #2 remains open: validate a stronger tool/filesystem boundary and actual
ChatGPT credential reuse without inheriting user tooling. The current controls
are a useful measured baseline but do not establish the desired closed-input
backend. Options are to harden the Codex execution sandbox or implement a direct
provider API backend if its independent authentication is acceptable. Do not
silently substitute an external provider for a disallowed project.

The official [non-interactive reference](https://learn.chatgpt.com/docs/non-interactive-mode)
describes schema output; the measured runtime fixtures establish the observations
above. No upstream implementation code was copied.
