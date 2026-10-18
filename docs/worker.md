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

### Local HTML dashboard

Start the local viewer and open the printed URL in a browser:

```sh
coding-memory --state-dir /absolute/path/to/state dashboard --serve
# Dashboard: http://127.0.0.1:8765
```

The viewer fetches a fresh read-only snapshot every five seconds. It shows the
last response time, flags unavailable sections, and retains the previous display
with a stale-data warning if a request fails or times out. It retries automatically
and resumes live updates after reconnection. Browser background throttling can
delay updates; returning to the tab checks freshness again. Requests time out
after ten seconds, and responses older than fifteen seconds are marked stale.
Snapshot and cycle timestamps use UTC; the connection indicator uses browser
local time. A completed refresh does not mean that all pipeline stages succeeded.

The listener binds only to `127.0.0.1`. Use `--port PORT` to choose another port,
or `--port 0` to select an available one. Only fixed GET routes for the page,
its local script and snapshots are served. Other Host/Origin values and requests
from other sites are rejected; responses are not cached. There are no job-control
endpoints, directory browsing, external assets, or CORS access. Stop the foreground
viewer with Ctrl-C or SIGTERM. This stops viewing only; the worker keeps running.
An occupied port fails explicitly without starting or changing a worker.

For a standalone snapshot without a running viewer:

```sh
coding-memory --state-dir /absolute/path/to/state dashboard > /private/path/status.html
open /private/path/status.html  # macOS; use a local browser on Linux
```

Without `--serve`, the command prints static HTML to stdout and requires no
JavaScript. Regenerate the file with the same command and reload the browser to
refresh it; reloading alone does not fetch new state. Worker activity is an
instantaneous lock observation and can change after generation.

The view includes actual worker lock ownership, the last cycle report, stage and
state counts by project, grouped attention reasons, recent successful extraction
outcomes (including `succeeded_empty`), and applied note IDs. Consolidation,
review and Vault application share the consolidation job state: `generating`,
`held`, `applying`, `conflict`, and `applied` are not separate independent queues.
Grouped attention reasons separate extraction evidence holds (absent, unreadable,
invalid, oversized) from consolidation context and proposal holds, and mark
evidence the Engine cannot fit as not retryable.
No candidates is a successful extraction outcome. Cycle attempts and consumed
candidates are not memory publication counts. Applied note records include
updates to existing memories; their order uses batch IDs because publication
timestamps are not recorded. They do not establish current Vault contents.

Each available empty section explicitly reports zero records. Unreadable state
or sections show `Unavailable`, without substituting zero or printing database
errors. The selected state directory is displayed so an incorrect `--state-dir`
can be identified. Use the same directory as the worker or MCP configuration;
the example paths are placeholders. Snapshots still produce HTML when state is
unavailable; a successful HTTP response or CLI exit is not a worker health check.
Older schemas with missing tables
must be prepared by the normal Engine setup before all sections are available.
Attention lists are limited to 100 groups per stage; recent outcomes and applied
notes are limited to 30 entries each. CLI status commands provide individual job
identifiers and further details for recovery.

Dashboard generation opens existing state read-only and uses a read transaction
for database metadata. It does not initialize state, claim jobs, change retries,
invoke models, read evidence, or open/modify the Vault. SQLite may use WAL/SHM
coordination files, as with other read-only connections. The worker lock check
reuses `worker-status` semantics and does not start a worker or change its lock
file. Captured content, candidate claims, prompts, provenance, file contents,
credentials, raw reason strings and model stderr are omitted. Known reason codes
are translated to static guidance; unknown reasons are withheld. HTML metadata
is escaped and the document loads no external resources. Keep the generated
file private: project names and memory IDs are operational metadata, and shell
redirection creates the file with your shell's permissions.

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
`held`, `exhausted`, application `conflict` or generation failure. A bundle with no
captured items reaches it without a model call. A completed cycle
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
