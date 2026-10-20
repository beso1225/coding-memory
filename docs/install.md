# Install and update

Release support starts with Apple Silicon Macs running macOS 15 or later.
Intel macOS, Linux, and Windows are not supported release targets yet.
Codex is installed and authenticated separately. Nix and Rust are not required
to run a release binary. The first stable release validates distribution;
broader runtime and quality validation remains ongoing.

## Install an archive

Choose a version from [GitHub Releases](https://github.com/beso1225/coding-memory/releases).
Prereleases must be selected explicitly; they are not the stable latest release.
The first stable release is `v0.1.0`; downloads become available only after its
release workflow succeeds.

Download anonymously with curl:

```sh
version=0.1.0
archive=coding-memory-aarch64-apple-darwin.tar.gz
curl --fail --location --remote-name \
  "https://github.com/beso1225/coding-memory/releases/download/v$version/$archive"
curl --fail --location --remote-name \
  "https://github.com/beso1225/coding-memory/releases/download/v$version/SHA256SUMS"
shasum -a 256 -c SHA256SUMS
tar -xzf "$archive"
mkdir -p "$HOME/.local/share/coding-memory/versions" "$HOME/.local/bin"
mv coding-memory-aarch64-apple-darwin \
  "$HOME/.local/share/coding-memory/versions/$version"
ln -sfn "$HOME/.local/share/coding-memory/versions/$version/coding-memory" \
  "$HOME/.local/bin/coding-memory"
"$HOME/.local/bin/coding-memory" --version
```

An authenticated GitHub CLI can fetch the same assets, and earlier `v0.1.0-rc.N`
prereleases, after `gh auth login`:

```sh
gh release download v0.1.0 --repo beso1225/coding-memory \
  --pattern coding-memory-aarch64-apple-darwin.tar.gz --pattern SHA256SUMS
```

Keep the whole version directory, including licenses, `release-notices/`, and
`index.mjs`. That file is the DeepSeek Harness capture bridge: cargo-dist places
each included file at the archive root, so a release install mounts
`$HOME/.local/share/coding-memory/versions/$version/index.mjs`. A copy that omits
it cannot capture DSH runs.

To enable DSH capture, mount the bridge with an overlay (DSH `0.2.1-rc.2` is the
verified revision):

```yaml
# coding-memory-capture.yml
- insert:
    - id: coding-memory-capture
      name: '/Users/you/.local/share/coding-memory/versions/0.1.2/index.mjs'
      config:
        executable: '/Users/you/.local/bin/coding-memory'
        stateDir: '/absolute/engine-state'
        dshHome: '/absolute/dsh-home'
        timeoutMs: 30000
```

```sh
dsh <profile> --patch /absolute/coding-memory-capture.yml
```

Use absolute paths; the plugin refuses relative ones at startup. The first turn
of an unregistered project is deferred, so register and reconcile the published
projection once as described in
[durable ingestion](https://github.com/beso1225/coding-memory/blob/main/docs/ingestion.md#deepseek-harness).
The overlay names a version-specific file: after an upgrade, repoint `name` at
the new version directory (or a stable symlink to the current `index.mjs`)
before restarting DSH.

Add `$HOME/.local/bin` to your shell PATH if needed. Checksums detect corruption;
they are not an independent publisher signature. Initial builds have no Apple
Developer ID signature or notarization. Honor macOS security prompts and inspect
the downloaded source/version rather than disabling Gatekeeper globally.

Homebrew support follows successful RC distribution validation. No formula or
tap is published by the initial pipeline. See [release operations](https://github.com/beso1225/coding-memory/blob/main/docs/releasing.md)
for the activation steps.

## Configure MCP and the Worker

Use the stable absolute path `$HOME/.local/bin/coding-memory` (expanded to your
actual home directory) in MCP and LaunchAgent configuration. Do not resolve it
to a version-specific path or point it into a build directory. Runtime state
and the Markdown Vault must remain outside installed version directories.

Follow [scoped retrieval](https://github.com/beso1225/coding-memory/blob/main/docs/retrieval.md) for MCP setup and
[background worker](https://github.com/beso1225/coding-memory/blob/main/docs/worker.md) for Worker configuration and independent Codex
login. `mcp-config --harness codex|opencode|dsh` renders the server entry for a
supported client; the engine never installs or edits that configuration for you.
Installing the CLI does not start a Worker or grant external transmission
permission. Activate the LaunchAgent explicitly after reviewing its configuration.

## Update

1. Record the current version and its symlink target. Quit/restart MCP clients
   around the update so no old MCP process retains the previous binary.
2. Unload the Worker with
   `launchctl bootout "gui/$(id -u)/com.beso1225.coding-memory"` if installed.
   Confirm it stopped, and stop any manually launched Worker and other writers.
3. Back up the entire state directory and authoritative Vault while writers are
   stopped. A live copy of just `state.sqlite` can omit SQLite WAL contents.
   Keep the Worker configuration too. Backups can contain sensitive evidence.
4. Download, verify, and unpack the new archive into a new version directory.
   Read its release notes for schema/configuration compatibility before changing
   the stable symlink as shown above.
5. Check `coding-memory --version`, then run `worker-status` and
   `runtime-check-status` with your explicit `--state-dir`. Revalidate the installed
   Codex with `runtime-verify --binary /absolute/path/to/codex` as needed.
6. If DSH capture is mounted, repoint the overlay's `name` at the new version
   directory (or its stable bridge symlink) and restart DSH. An overlay left on
   the previous version runs the old bridge against the new engine, and removing
   that version breaks plugin loading entirely.
7. Reload the existing LaunchAgent with `launchctl bootstrap "gui/$(id -u)"`
   followed by its plist path. Check `launchctl print` and `worker-status`, then
   restart MCP clients and confirm memory search works.

If your existing plist references a version-specific binary, unload it first
and generate a replacement with `service-plist --executable` pointing to the
stable path. The generator refuses to overwrite an existing plist: retain the
old file as a backup and generate a new file explicitly.

## Recover

Stop the Worker, MCP clients, and other writers first. If the release notes
confirm backward-compatible state, restore the symlink to the previous retained
version and reload the Worker. Otherwise restore the matching stopped-state
backup and configuration before running the previous binary. Restoring the
Vault also discards subsequent edits, so preserve its current copy and account
for changes made after the backup. Never assume a binary downgrade reverses a
database migration or a Vault update. Revalidate the local Codex runtime after
recovery if the binary/fixture identity changed.
