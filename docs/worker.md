# Background worker

Issue #7 connects reconciliation, capture, extraction, reviewed consolidation and
interrupted Vault application. One process owns each state directory through an
OS `flock`, explicitly unlocked at final owner release even if a concurrent
child inherited its descriptor; a PID file alone is not proof that a worker is running. Foreground and
launchd operation use the same configuration and pipeline.

## Configuration and preparation

Use persistent absolute paths. Keep private state outside collected software
repositories and the Vault. Register each source project/worktree as described in
[ingestion](ingestion.md); collection remains scoped to explicit registered
directories and their enablement boundaries. A source home is the development
runtime's home, never the Engine's generation home. The latter has a separate,
human-performed login as described in [extraction](extraction.md).

Save a private `worker.json` outside Git, adapting this example:

```json
{
  "schema_version": 1,
  "vault": "/absolute/path/to/coding-memory-vault",
  "sources": [{
    "project": "/absolute/path/to/software-project",
    "codex_home": "/absolute/path/to/development-codex-home",
    "codex_binary": "/absolute/path/to/codex"
  }],
  "generation_binary": "/absolute/path/to/codex",
  "models": {
    "extraction": "gpt-6.1-sol",
    "consolidation": "gpt-6.1-sol",
    "review": "gpt-6.1-sol"
  },
  "poll_seconds": 30,
  "timeout_seconds": 300,
  "min_ready": 5,
  "max_age_seconds": 600,
  "executable_path": ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
}
```

The review uses a fresh context even when its model matches the proposer. External
transmission still requires explicit permission per project; worker configuration
does not grant it. Generation credentials are never copied from the source home.
Specify additional executable search directories if a Codex wrapper needs Node
or another launcher. launchd receives this explicit PATH and runs no interactive
shell; direnv and shell startup files cannot supply missing dependencies.

Configuration rejects unknown fields, duplicate project/store pairs, relative or
missing executables, unregistered projects, and Engine-owned collection stores.
Limits are 1–16 sources, 1–300 seconds between cycles, 1–600 seconds per generation,
1–20 ready candidates and 1–86400 seconds of oldest-candidate age. Omitted timing,
trigger and PATH fields use the values above. `loopback` is reserved for isolated
synthetic protocol probes.

```sh
coding-memory --state-dir /absolute/path/to/state worker \
  --config /absolute/path/to/worker.json --once
coding-memory --state-dir /absolute/path/to/state worker-status
coding-memory --state-dir /absolute/path/to/state worker \
  --config /absolute/path/to/worker.json
```

`--once` runs one bounded cycle. Continuous operation sleeps after completing each
cycle. Each cycle attempts at most eight captures per source, four extractions
per distinct project and one consolidation batch per project. The five-candidate
or ten-minute consolidation trigger remains per project. Processing time and
backlog add latency; ten minutes is a trigger, not a publication deadline. A
reader failure does not prevent processing already captured evidence.

Continuous workers also execute independent, durable reader verification jobs.
New binary/fixture hashes and seven-day expiry queue a synthetic compatibility
check; collection is allowed provisionally while pending. A failed check blocks
new collection by that reader until a check passes. `--once` only queues this work.
Use `runtime-check-status` or `runtime-verify --binary BINARY [--retry]` for manual
inspection and verification. See [App capture](app-capture.md) for the admission
floors, failure policy, audit fields and weekly Actions coverage.

## Inspection and recovery

`worker-status` returns actual lock activity, the last cycle report, grouped
counts for runs/extraction jobs/candidates/consolidation jobs, and bounded grouped
failure reasons, plus `runtime_checks` metadata. Cycle counters count attempts, not successful memories. Use
`status`, `extraction-status --project-id PROJECT`, `candidates --project-id PROJECT`
and `consolidation-status` to inspect individual records.
New consolidation records include bounded sizing diagnostics. Use
`consolidation-preflight --vault VAULT --project-id PROJECT --batch BATCH` for a
model-free preview of held work before deliberate retry; it does not release jobs
or rebuild the live index. See [consolidation](consolidation.md#held-batch-preflight-issue-20-final-increment).

Successful empty extraction is `succeeded_empty`; it is distinct from `queued`,
`held`, `exhausted`, application `conflict` or generation failure. A completed cycle
may contain failures or held work, so process exit zero is not a quality signal.
Logs contain cycle counters and static stage codes, rather than captured prompts
or model stderr. launchd log rotation is an operator responsibility.

Worker-acquired capture/extraction claims are recorded transactionally with their
lease tokens. On restart, only matching abandoned worker claims are reclaimed;
manual CLI claims and newer tokens are preserved. Late results fail the normal
fence. Extraction keeps its existing three-attempt bound. Interrupted worker
consolidation becomes held and its reserved candidates remain protected.

Timeout, process failure and worker interruption allow automatic consolidation
retry after at least ten seconds, only for configured projects. A candidate's
third batch participation exhausts automatic retry conservatively, including
earlier manually generated batches. Permanent policy/authentication/output/review
failures remain held. Deliberate `consolidation-retry BATCH` or
`extraction-retry STORE THREAD TURN` is available after fixing the cause; deliberate
retry can override exhaustion. See the corresponding stage documents for details.

Recorded `applying` batches recover before collection. The recorded Vault must
match the configured Vault. Unexpected file revisions become conflicts and
require explicit inspection and `vault-recover --vault VAULT BATCH`; the worker
does not resolve them by overwriting content. MCP publication barriers remain in
effect during incomplete application.

SIGINT/SIGTERM cancels current work, kills generation's owned process group and
releases ownership. The durable claim is recovered by the next worker. There is
no automatic evidence cleanup or Vault Git commit/push.

## macOS login startup

Build and place the Engine binary at a persistent absolute path, then generate a
user LaunchAgent. Substitute paths below; the command refuses to overwrite an
existing plist. This generates a file but does not install or load a service.

```sh
mkdir -p "$HOME/Library/LaunchAgents"
coding-memory --state-dir /absolute/path/to/state service-plist \
  --config /absolute/path/to/worker.json \
  --executable /absolute/path/to/coding-memory \
  --output "$HOME/Library/LaunchAgents/com.beso1225.coding-memory.plist"
plutil -lint "$HOME/Library/LaunchAgents/com.beso1225.coding-memory.plist"
launchctl bootstrap "gui/$(id -u)" \
  "$HOME/Library/LaunchAgents/com.beso1225.coding-memory.plist"
launchctl print "gui/$(id -u)/com.beso1225.coding-memory"
```

`RunAtLoad` starts at login/bootstrap. Unsuccessful process exit restarts with a
30-second throttle; clean shutdown stops until login or explicit loading. Stdout
and stderr go to `worker.stdout.log` and `worker.stderr.log` inside private state.
Stop with `launchctl bootout gui/UID/com.beso1225.coding-memory`, substituting UID.
Unload before replacing a plist or changing its PATH. Ensure only one service
references the same state directory.

## Validation

Offline checks cover singleton lifetime, token-fenced recovery, manual claimant
preservation, bounded retries/deadlines/project scope, unavailable readers,
configuration, signal cancellation and XML/argv escaping. Existing ingestion and
Vault tests cover delayed history, duplicate delivery and interrupted application.

```sh
nix develop -c pkf run check
nix develop -c python3 experiments/worker/runtime_probe.py \
  --engine target/debug/coding-memory --codex /absolute/path/to/codex --launchd
```

The opt-in probe uses isolated synthetic state/Vault/source histories and a
loopback model. On Codex CLI 0.162.1, it completed all pipeline stages, deduplicated
a second cycle, rejected duplicate ownership, released the lock on SIGTERM, and
bootstrapped/removed a temporary user LaunchAgent. No personal service or Vault
was installed or modified. Actual login persistence across a machine reboot is
not claimed by this temporary lifecycle probe. See [quality evaluation](quality-evaluation.md)
for the separate authenticated model experiment.
