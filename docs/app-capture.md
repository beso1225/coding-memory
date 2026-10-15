# Local App capture and compatibility checks

Issue #16 identified a long-lived App chat whose stored creation `cliVersion` was
`0.162.0-alpha.2`, although the installed App executable was
`0.162.0-alpha.17.2` and the configured CLI reader was `0.162.1`. The previous
exact version gate rejected it before reading turns. Updating the executable does
not rewrite a chat's creation metadata. This report and its fixtures contain no
personal thread IDs, transcript contents or credentials.

## Provisional admission

Capture now admits reader versions at or above `0.162.0-alpha.17.2`, and history
creation versions at or above `0.162.0-alpha.2`. Comparisons use Semantic Versioning
precedence, including numeric prerelease identifiers; malformed or missing versions
are held. Admission allows an attempt, not a promise that every future format works.
The selected history must still satisfy paging, native identities, terminal status,
timestamps and unambiguous per-turn project attribution. Those checks remain strict.

An admitted reader may capture while its verification is pending or stale. Each
new evidence bundle records `capture_runtime`: reader executable SHA-256, version,
fixture hash, `provisional`/`verified` status and the last check time when available.
Existing bundles without this optional field remain readable. The audit describes
reader compatibility at capture time; it does not certify knowledge correctness or
all creation versions, and later checks do not rewrite immutable evidence.

Generation uses a separate exact gate (`0.162.1` and `0.162.0-alpha.17.2`). A capture
fixture does not validate generation's tool-denial isolation, so newer reader
admission alone does not admit a new extraction/consolidation executable.

## Worker and Actions verification

The Worker observes configured readers on collection cycles and queues a check for
a new executable hash, a changed fixture, or a result older than seven days. The fixture ID covers all Rust sources, build
configuration/lockfile, compiler, target and enabled features, so transitive changes
and newly added modules invalidate prior verification. A
backward clock change also queues revalidation. Continuous workers execute checks
in a separate asynchronous job using durable, token-fenced SQLite leases. Normal
collection can continue during a pending check. `worker --once` queues checks but
does not run this background loop; use the manual command below when needed.

A failed check blocks new reconciliation/capture by that reader, with
`runtime_validation_failed` in the cycle report. Already captured evidence,
extraction, consolidation and retrieval remain available. A known failure remains
blocking across fixture changes until a new check passes. Failures can be retried
manually, or when next due after seven days. Interruption leaves a recoverable
180-second lease. The complete check, including fingerprints, has a 150-second
deadline; each fingerprint is bounded to 30 seconds and the synthetic probe to
90 seconds. The fixture reuses the pinned, prechecked identity for its producer
and readers; executable fingerprints surround it rather than being repeated inside
it. Hashing runs outside the async collection executor. Version probes also use a
cleared environment and the selected temporary HOME during synthetic checks.

```sh
coding-memory --state-dir /absolute/engine-state runtime-check-status
coding-memory --state-dir /absolute/engine-state runtime-verify \
  --binary /absolute/path/to/codex
# Force a fresh check after fixing a failure; does not clear failure before success.
coding-memory --state-dir /absolute/engine-state runtime-verify \
  --binary /absolute/path/to/codex --retry
```

The manual command processes eligible queued checks in this state directory and
requires a fresh passed result for the selected binary. An active worker lease
cannot be overridden by `--retry`; an expired lease can be reclaimed explicitly. Moving an identical binary invalidates its old in-flight
claim and queues verification at the new path. Late results from the old path
cannot change the recorded result.
`worker-status` also includes bounded `runtime_checks` metadata. Executable and
fixture hashes identify local results; a wrapper's hash does not fingerprint its
interpreter or all dependencies, which periodic rechecks help cover.

The Runtime compatibility Actions workflow runs on PRs, manually, and every Monday
at 03:17 UTC. It downloads the latest official Codex CLI release and runs the same
Rust fixture on an isolated macOS runner. Actions failure provides an early signal
for upstream changes; success does not certify a user's installed executable hash.
The App-bundled executable is checked locally rather than downloaded by this job.

The fixture creates two synthetic turns through a private producer, then captures
each with a separate cold read-only app-server and repeats the read. It checks turn
separation, message projection, completion and stable native identities. A loopback
provider supplies fixed responses without authentication or paid-model requests.
Temporary HOME/CODEX_HOME, scrubbed environment, disabled hooks/plugins/analytics,
Engine-owned source exclusion and process-group cancellation protect normal state.
Temporary fixture files are removed on completion or cancellation.

This fixture does not establish all tool families, forks, compaction, attachments,
App GUI restart/reopen behavior or semantic quality. Offline adapter tests cover
additional malformed shapes and attribution boundaries. Future versions that fail
those checks are held even if their version passes the admission floor.

## Metadata-only diagnosis

```sh
coding-memory --state-dir /absolute/engine-state capture-diagnose \
  --codex-home /absolute/development-codex-home \
  --codex-binary /absolute/path/to/codex \
  --project /absolute/registered-project --thread THREAD_ID
```

The command initializes an independent cold app-server, calls `thread/read` with
`includeTurns=false`, and closes it. It does not read items, claim/requeue work or
call a model. Common CLI initialization still opens private state and its schema;
a disposable state path can be used. Output contains only reader version, bounded
creation-version string, `history_version_supported` and a static reason. Malformed
version strings are withheld. The support flag describes admission alone; the old
`0.162.0-alpha.2` creation version now passes this gate.

## Held-work reasons and retry

New attempts persist specific bounded codes rather than `unsupported_format`:

| Reason | Boundary |
| --- | --- |
| `missing_history_version`, `unsupported_history_version` | Thread creation metadata |
| `unsupported_history_method` | Required read/list RPC unavailable |
| `invalid_history_page_data`, `invalid_history_page_cursor` | Paging response shape |
| `invalid_turn_identity`, `unsupported_turn_status` | Selected turn metadata |
| `missing_turn_start`, `invalid_turn_timestamps` | Completed turn timing |
| `invalid_item_identity`, `missing_item_projection`, `item_turn_mismatch` | Selected-turn item projection |
| `missing_source_path`, `missing_turn_context`, `missing_turn_cwd`, `conflicting_turn_context` | Local source attribution |

Pending completion still defers. Inherited/nonhuman turns remain excluded. Source
size limits, project identity, baseline and Engine-owned exclusions still apply.
Reasons contain no messages, raw stderr or native contents. Existing held rows keep
their reason until deliberately retried; there is no automatic bulk backfill.

```sh
coding-memory --state-dir /absolute/engine-state retry STORE THREAD TURN
```

After installing the update, retry an individual held key from `status`, then let
the Worker capture it. Retry does not override baseline/owned-store exclusions,
required history fields or a failed reader check, and cannot requeue successful
runs. An old admitted chat may still encounter a shape failure; inspect its specific
reason rather than assuming admission guarantees capture.

## Validation

`pkf run check` covers SemVer floors, metadata-only diagnostics, safe shape reasons,
provisional evidence audits, failed-reader capture blocking, weekly expiry, fixture
changes, deduplicated jobs and fenced lease recovery, alongside existing capture
and generation protections.

The Rust `runtime-verify` fixture passed locally on CLI `0.162.1` and the App-bundled
`0.162.0-alpha.17.2`. The isolated Worker probe also checks continued cycles during
background verification and SIGTERM ownership release. Earlier human-run App GUI
checks established approved hooks and cold reads for fresh App histories. A
subsequent human-run fresh App GUI directory-check turn was captured as
`ready`/`completed` at creation version `0.162.0-alpha.17.2`, and extraction finished
`succeeded_empty`, as expected for a transient directory report. This used the
existing production Engine, so it confirms the fresh-chat route rather than the
new provisional policy. Capture of the diagnosed old chat after installing this
update remains an operator check.
