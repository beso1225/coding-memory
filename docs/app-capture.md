# Local App capture diagnostics

Issue #16 identified a long-lived App chat whose stored `cliVersion` was
`0.162.0-alpha.2`, even though the installed App-bundled executable was
`0.162.0-alpha.17.2`. The configured CLI reader was `0.162.1`. Capture rejected the
thread creation version before reading its turns or items. Updating an executable
does not rewrite an existing chat's creation metadata.

The public diagnosis contains only versions and static reasons. No personal thread
IDs, transcript contents, credential files or raw source artifacts are included in
this report or its synthetic regression fixtures.

## Supported boundary and workaround

Reader executables and history creation versions are separately checked against
`0.162.1` and `0.162.0-alpha.17.2`. `0.162.0-alpha.2` remains unsupported: the old
history contract has not been independently established. The Engine does not infer
missing identity/timestamp/cwd fields, loosen the gate, patch native databases or
upgrade old rollout metadata to make a chat appear supported.

For ongoing work, start a new local App chat after updating the App, in an already
registered project directory. Start after the enablement second; there is no
historical backfill. Keep the existing collection home and registered source in
worker configuration. The reader can discover supported App histories in the same
store through reconciliation, without starting or resuming the App's thread.
Separate clones/subdirectories and alternative App state homes still need the
explicit registration/source configuration described in [ingestion](ingestion.md).

Do not treat a supported version alone as successful collection: verify completed
runs become `ready`, failed/interrupted runs are recorded as such, and missing or
ambiguous history remains held. The earlier human-run App GUI experiment established
approved hooks and cold reads for new App-created histories at the supported bundled
version. A new GUI daily-use confirmation for this change is pending; the current
runtime probe below tests the bundled executable, not GUI restart/reopen behavior.

## Metadata-only diagnosis

```sh
coding-memory --state-dir /absolute/engine-state capture-diagnose \
  --codex-home /absolute/development-codex-home \
  --codex-binary /absolute/path/to/codex \
  --project /absolute/registered-project --thread THREAD_ID
```

The selected reader must itself be supported. The command initializes an independent
cold app-server and calls `thread/read` with `includeTurns=false`, then closes it.
It does not read items, capture evidence, claim/requeue work or call a model. The
common CLI state initialization still creates/opens the private state directory
and durable schema; a disposable state path can be used for standalone diagnosis.
Output contains the reader version, bounded creation-version string,
`history_version_supported` and a static reason. Other thread metadata is omitted.
A malformed version string is withheld from output. A true support flag validates
only the version gate, not the remaining capture contract or project attribution.

For the diagnosed old chat, the result is:

```json
{
  "reader_version": "0.162.1",
  "history_creation_version": "0.162.0-alpha.2",
  "history_version_supported": false,
  "reason": "unsupported_history_version"
}
```

## Held-work reasons and retry

New capture attempts persist specific bounded reason codes instead of combining
these findings as `unsupported_format`:

| Reason | Boundary |
| --- | --- |
| `missing_history_version`, `unsupported_history_version` | Thread creation metadata |
| `unsupported_history_method` | Required read/list RPC unavailable |
| `invalid_history_page_data`, `invalid_history_page_cursor` | Paging response shape |
| `invalid_turn_identity`, `unsupported_turn_status` | Selected turn metadata |
| `missing_turn_start`, `invalid_turn_timestamps` | Completed turn timing |
| `invalid_item_identity`, `missing_item_projection`, `item_turn_mismatch` | Selected-turn item projection |
| `missing_source_path`, `missing_turn_context`, `missing_turn_cwd`, `conflicting_turn_context` | Local source attribution |

Pending completion still defers, inherited/nonhuman turns remain excluded, and
source-size limits, project identity, baseline exclusions and Engine-owned
exclusion are unchanged. Reasons contain no messages, raw stderr or native content.
Old held rows retain their existing reason until deliberately retried; no automatic
bulk retry or historical reprocessing is performed by this change.

```sh
coding-memory --state-dir /absolute/engine-state retry STORE THREAD TURN
```

Use an individual run's key from `status`, only after correcting a transient source
problem or validating new compatibility support. Let the worker retry it or invoke
`capture` with the corresponding source. Retry cannot override the version gate or
baseline/owned-store exclusions, and does not requeue successful runs. An old
`0.162.0-alpha.2` chat will remain held; a new supported chat is the present workaround.

## Validation

`pkf run check` covers the old-version hold before any turn read, metadata-only
CLI diagnosis without queue mutation/generation, specific shape failures, protected
output and the existing identity/readiness/ownership boundaries.

```sh
nix develop -c python3 experiments/codex_capture/ingestion_probe.py \
  --engine target/debug/coding-memory \
  --codex /Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex \
  --turns 2
```

On bundled `0.162.0-alpha.17.2`, two isolated synthetic completed turns were captured
through separate cold reader processes. Repeated reconciliation inserted no
duplicates, and each evidence bundle contained only its selected human instruction.
The source uses a loopback provider without authentication or paid-model requests.
It neither controls the App GUI nor modifies personal histories/Vaults.
