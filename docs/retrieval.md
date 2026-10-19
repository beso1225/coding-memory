# Scoped Retrieval and MCP

Work item: [#6](https://github.com/beso1225/coding-memory/issues/6).
Implemented on 2026-10-14 (Asia/Tokyo). This stage indexes existing accepted notes
and retrieves them; it does not generate knowledge or update vault files.

## Note contract

The version-1 Markdown format is defined by `notes::Metadata`, with a round-trip
fixture at [queue.md](../tests/fixtures/vault/queue.md). Each file has YAML frontmatter
and a nonempty Markdown body. Stable IDs do not depend on titles or relative paths.
No existing vault content is rewritten by parsing or indexing.

Required metadata includes `schema_version`, `id`, `title`, `summary`, explicit
`scope`, `status`, `conditions`, `verification`, `evidence`, `review`, `related`,
and `created_at`/`updated_at`. Timestamps are Unix seconds. Scope is either
`kind: project` with a nonempty `project_id`, or explicit `kind: global`; omission
does not imply global scope. IDs use ASCII letters, digits, hyphens, and underscores,
up to 128 bytes. Registered project IDs come from Issue #3's Git common-directory
identity; short identifiers in tests are synthetic.

An active note has no successor. A superseded note names a distinct successor and
requires deliberate historical retrieval. Evidence locators pair a canonical lowercase SHA-256 bundle
digest with a native item identity. Verification is `observed`, `user_stated`, or
`unverified`; review acceptance does not promote that state. Review records carry
an accepted decision, backend/model identity, prompt version, and review timestamp.
Notes with missing required fields, unsupported schemas, or non-accepted reviews
are not indexed. This validates shape, not semantic truth or link relationships.
Issue #5 supplies publication and semantic review validation.

Files are bounded to 64 KiB, frontmatter to 16 KiB, summaries to 512 bytes, and
other metadata collections/text to the limits in `Metadata::validate`. CRLF input
is accepted. Duplicate valid IDs exclude every conflicting file from a rebuild.
This is a coding-memory note schema, not an importer for arbitrary Obsidian notes.

## CLI

Use a dedicated local engine state directory and an explicit vault location:

```sh
coding-memory --state-dir /absolute/engine-state index-rebuild \
  --vault /absolute/coding-memory-vault

coding-memory --state-dir /absolute/engine-state search \
  --vault /absolute/coding-memory-vault --project-id REGISTERED_PROJECT_ID \
  'State::claim_store'

coding-memory --state-dir /absolute/engine-state read \
  --vault /absolute/coding-memory-vault --project-id REGISTERED_PROJECT_ID \
  queue-leases
```

Search and read default to the selected project's active notes and explicit global
active notes. Use repeated `--other-project ID` options to deliberately expand
project scope, and `--include-historical` for superseded notes. Read enforces the
same scope rules, even when the caller already knows a note's ID. The default
search limit is five, with a maximum of ten.

`index-rebuild` returns indexed count and invalid-note diagnostics. It scans
non-hidden Markdown files recursively and skips symlinks, including directory
symlinks. There is a 10,000-entry scan bound and a 64 MiB aggregate readable-note
bound. Fatal resource/directory and per-file I/O errors leave the old index intact; malformed notes
are reported and omitted from the new index. Renamed notes retain their identity
once rebuilt. New or changed content requires explicit rebuild/refresh at this stage.

## Index and query behavior

`state.sqlite` remains durable processing state. `search.sqlite` contains only the
rebuildable FTS index and its note/vault binding, so index rebuilding cannot delete
jobs, exclusions, or publication barriers. Rebuilds replace derived rows in one
short SQLite transaction. Readers use WAL and read-only connections; each request
opens a current view, rather than keeping a stale connection after rebuild. Pending
checks reuse one state connection and a cached statement throughout each request.

The index uses [SQLite FTS5](https://www.sqlite.org/fts5.html), `unicode61` with
`remove_diacritics 2`, and BM25 with extra title/summary weight. Lower BM25 scores
rank first; a score is lexical relevance, not model confidence. Index fields are
title, summary, and body. This version does not add stemming, embeddings, synonyms,
Japanese segmentation, or query translation.

Queries are literal text: whitespace-separated chunks become quoted FTS phrases
joined by AND, and embedded quotes are escaped. For example, `State::claim_store`,
`sync_all`, and `src/state.rs` retain their tokenizer-separated phrase order. `OR`,
`NOT`, wildcard characters, and column syntax are never passed through as operators.
All SQL values are bound parameters; only engine-generated placeholder counts enter
SQL syntax. Empty/punctuation-only input, NUL, queries over 1,024 bytes, more than
16 searchable chunks, and result limits outside 1..10 return errors.

Search returns summaries, stable IDs, applicability, evidence/review metadata,
revision hashes, and lexical scores. Results are limited to approximately 64 KiB
of structured JSON, with at most 1,000 candidate entries checked. Response fields
`omitted_unavailable` and `scan_limit_reached` expose incomplete available results.
Read returns the exact authoritative Markdown plus parsed metadata, revision, and
local evidence availability. Availability checks the bundle hash and item locator;
a missing/corrupt bundle does not erase the note or invent verification.

## Publication and refresh boundaries

A note's retrieval availability is separate from its active/superseded status and
verification. Every lookup checks its authoritative file hash against the indexed
revision. Missing, changed, or symlinked files are unavailable pending refresh.
Other unchanged notes remain usable; generation failure does not disable the index.

Durable `pending_notes` entries in `state.sqlite` withhold affected identities from
both search and read, including in separately started MCP processes. Issue #5 must
mark affected notes pending **before** changing files, retain the barrier across
interrupted application, and clear it only after application finalization and index
refresh. `State::mark_note_pending`/`clear_note_pending` establish this integration
contract. Index rebuild never clears these barriers. This stage does not implement
the publication journal or claim whole-batch filesystem/database atomicity.

## MCP setup

The stdio server uses the [official Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk),
`rmcp` 3.5.1. It exposes only `memory_search` and `memory_read`, annotated read-only.
Both call the same service as the CLI. The tools cannot rebuild the index, change
notes, start generation, or access a user-supplied file path. Stdout contains only
MCP protocol messages.

The server is harness-agnostic, but its declaration is not: each client names the
same command in its own format. `mcp-config` renders that entry from one set of
inputs, resolves the state directory and vault to absolute canonical paths, records
the running binary unless `--executable` is given, and writes nothing:

```sh
coding-memory --state-dir /absolute/engine-state mcp-config \
  --harness opencode \
  --vault /absolute/coding-memory-vault \
  --project-id REGISTERED_PROJECT_ID
```

`--harness` is `codex`, `opencode`, or `dsh`. `--server-name` overrides the default
`coding_memory`; it must be a bare identifier of ASCII letters, digits, `-`, and `_`,
at most 32 bytes (the tightest bound, from DeepSeek Harness). The state directory
and vault must already exist. Build the index first (`index-rebuild`), and replace
`REGISTERED_PROJECT_ID` with that project's registered ID.

| Harness | Output | Placement |
| --- | --- | --- |
| `codex` | TOML table | `[mcp_servers.<name>]` in `$CODEX_HOME/config.toml` or a project `config.toml` |
| `opencode` | JSON document | `opencode.json` in the project root, merged into `mcp.servers`; the global config works too, but still binds one project |
| `dsh` | Cordis overlay | `dsh <profile> --patch <file>` (repeatable), or merge the `insert` entry into `$DSH_HOME/cordis.patch.yml` or `$DSH_HOME/profiles/<name>/cordis.patch.yml` |

The `dsh` overlay leaves `failOnStartupError` at the `dsh-mcp-client` default
(`false`): a server that cannot be reached logs an error and contributes no tools
while the harness starts anyway. Add the field to the generated entry to reject
that entry's activation instead, which is the form the runtime probe below
verifies:

```yaml
config:
  serverName: coding_memory
  transport: stdio
  command: /absolute/coding-memory
  args: [...]
  failOnStartupError: true
```

For example, `--harness codex` prints:

```toml
[mcp_servers.coding_memory]
command = "/absolute/coding-memory"
args = ["--state-dir", "/absolute/engine-state", "mcp", "--vault", "/absolute/coding-memory-vault", "--project-id", "REGISTERED_PROJECT_ID"]
```

Tool names differ per client. Codex exposes this server as the `mcp__coding_memory`
namespace, with function calls carrying the namespace and the unprefixed tool name.
DeepSeek Harness connects through `@deepseek-ai/dsh-mcp-client` and names each tool
`mcp__coding_memory__memory_search`; it scrubs credential-shaped and `DSH_*`
variables before launching the child, which this server does not need. OpenCode
registers `<name>_<tool>`, for example `coding_memory_memory_search`.

The engine does not install any of this; paste or merge the entry yourself, and do
not reuse a server bound to one project as the default for an unrelated project's
work. Configure that project's ID, or deliberately select the other project in a
tool request. The shared vault remains local and read-only to MCP.

`memory_search` accepts `query`, optional `limit`, `other_projects`, and
`include_historical`. `memory_read` accepts `id` and the same scope-expansion options.
Tool failures return `isError: true`; empty search results are successful. Results
include both text JSON and structured content for client compatibility.

The server supplies [agent instructions](agent-memory-instructions.md): when to
search, how to form English queries, and how to assess conditions and support before
reuse. No separate translation/generation call is needed for retrieval.

## Validation and installed-runtime probe

Deterministic tests cover Markdown round trips, explicit scope, global and historical
selection, identifiers/paths, literal operators, malformed queries, size/limit checks,
duplicate identities, symlink/deletion handling, stale file hashes, pending-publication
exclusion, local evidence availability, and durable-state preservation. A separate
stdio client negotiates MCP, lists exactly two read-only tools, searches, reads,
and receives a tool-level error for invalid input. Queue work/failure and pending
publication leave unaffected retrieval available.

Installed Codex `0.162.1` completed real `memory_search` and `memory_read` MCP calls
against the synthetic fixture. Both returned `queue-leases`; read included the known
`State::claim_store` text and explicitly missing local fixture evidence. The runtime
exposes this server as namespace `mcp__coding_memory`, with function calls carrying
`namespace` and the unprefixed tool name. Flattened names from older assumptions
were rejected, so the reproducible probe uses the observed current form.

The generated entries were also checked against installed clients. OpenCode
`2.0.30` loaded a project `opencode.json`, reported `coding_memory` as `connected`
through both `opencode mcp list` and `GET /api/mcp` for that directory, and held a
live `coding-memory … mcp …` child of `opencode serve`. DeepSeek Harness
`@deepseek-ai/dsh` `0.2.0-rc.2` composed the generated overlay with
`web --dump-config --patch`, then activated it: with `failOnStartupError: true` the
real command started the documented stdio probe and serving processes without an
error, while a deliberately missing command failed activation with "initial
connection or tool synchronization failed".

A local DeepSeek Harness `0.2.1-rc.2` checkout then completed a model-driven call in
the browser UI: a `web` session routed through a non-DeepSeek route (Ollama Cloud,
model `deepseek-v4.1-flash:1017`) invoked `mcp__coding_memory__memory_search` and
returned the project-scoped result, with `scope.project_id` matching the bound
project. That covers the client-side connection, tool discovery, and one real
answer, not the model's autonomous query selection or the usefulness of what it
returns.

DeepSeek Harness patches Node.js internals through a native addon that matches
compiled instruction patterns, so it can reject a Node.js build it does not
recognize: a self-built Node (for example a nixpkgs `nodejs-slim`) fails host
preparation with `node-addon-require-builtin unsupported: Unsupported/no-getter`
before any plugin mounts, while the official builds work (verified on `22.23.3`,
`24.20.0`, and `24.21.0`). `--dump-config` still succeeds under the rejected build,
so only a real profile boot exercises the addon.

Repeat after building:

```sh
nix develop --command python3 experiments/codex_capture/mcp_probe.py \
  --engine /absolute/coding-memory/target/debug/coding-memory --codex /absolute/codex
```

The probe uses fresh synthetic runtime state/vault/engine directories and a loopback
Responses provider, disables capture hooks/notify in the test exec, and does not
read personal histories or use an authenticated/paid model. The provider scripts the
tool calls: this verifies runtime/MCP routing and actual returned note content,
not an LLM's autonomous query selection, knowledge usefulness, or semantic truth.
Those quality checks and cross-session application remain Issue #7.
