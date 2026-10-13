# Codex Generation Isolation Experiment

Date: 2026-10-14 (Asia/Tokyo). Work item: #2.

This is an installed CLI 0.162.1 experiment, not the production generation
backend. The three roles use separate exec processes and distinct configurable
model names against the synthetic loopback Responses provider. The loopback
probe uses no personal history or authentication. A subsequent
authenticated probe reused the existing Codex login in a private generation home
with synthetic evidence only; its three gpt-6.1-sol contexts returned valid
structured output. These checks establish plumbing, not knowledge quality.

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

## Stronger execution policy

A dedicated generation home containing only a vetted PreToolUse deny hook
blocked the same exec_command request. The Hook recorded the attempted call,
and no command-execution item was emitted; the unguarded control did execute
the command. This configuration enables lifecycle hooks for execution policy,
not for collection. Explicit notify=[] and the registered engine-owned namespace
remain necessary. Trust bypass applies only to the generated, reviewed policy
script in that fresh home. Do not enable bypass in a regular development home.

The authenticated probe used that policy with web_search disabled, user config
and rules ignored, plugins disabled, and read-only execution. Existing auth was
copied into an exclusively created mode-0600 file in a private state directory.
The credential context cleans up partial copies and ordinary completed copies.
If Codex refreshes the credential, the changed copy is preserved privately as
`refreshed-auth.json` and reported in `refreshed_credentials_preserved`; the probe
does not overwrite the existing login or discard a potentially rotated token.
All three fresh contexts returned schema-shaped output without requesting tools.
This does not prove interception of every tool family or managed configuration.
The command-denial fixture is the positive policy test; the authenticated runs
are the credential/schema tests.

To repeat authenticated tests, explicitly opt in to external synthetic requests:
`python3 experiments/codex_generation/authenticated_probe.py --allow-external`.
Without that flag it returns held_external_policy before reading credentials or
creating the generation home. It defaults to ~/.codex/auth.json; `--auth-source`, `--binary`, and `--model`
can select the installed environment explicitly. The script is import-safe,
uses the shared schema/policy helpers, and persists a report even on exceptions.
Timeout and malformed output are recorded outcomes; it refuses stale output paths.
Raw results stay outside Git; fixtures contain only synthetic results and IDs.

## Remaining implementation work

The user selected further Codex exec hardening rather than switching to a direct
API. The experiment now demonstrates credential reuse, policy-based command
denial, three independent contexts, collector exclusion, and output handling.
The Rust implementation must make namespace registration durable, integrate
credential refresh safely with the regular login, and validate other tool families and managed
configuration before unattended use. Do not claim a complete filesystem or
network isolation guarantee from one denied command. Do not silently substitute
an external provider for a disallowed project.

The official [non-interactive reference](https://learn.chatgpt.com/docs/non-interactive-mode)
describes schema output; the measured runtime fixtures establish the observations
above. No upstream implementation code was copied.
