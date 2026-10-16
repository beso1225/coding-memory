# Release operations

The initial channel is a GitHub Release containing an Apple Silicon macOS 15+
archive, SHA256SUMS, and a cargo-dist manifest. Cargo-dist 0.33.0 packages the
archive; repository-owned GitHub Actions workflows control validation and
publication. `ci = []` intentionally disables cargo-dist workflow generation.
The packaging tool's download is pinned by version and SHA-256.

## Release workflow

Every pull request builds the actual archive, generates dependency notices, and
tests the unpacked binary with a fresh HOME and system-only PATH. This exercises
version/help, SQLite FTS retrieval, MCP negotiation and search, system library
linkage, architecture, deployment target, and required archive contents.
Personal Vaults, state, credentials, and Codex binaries are not release inputs.

On main, a Cargo.toml version change triggers publication after the full Nix
development checks and archive smoke checks pass. A push that changes the lock
file or changelog without changing the version only builds a dry run. Updating
an existing released version does not silently replace its assets. Workflows
run against their triggering commit, even if main advances while they build.

For a release PR:

1. Choose the next `X.Y.Z` or `X.Y.Z-rc.N` version. Use `v0.1.0-rc.1` first to
   validate installation before announcing the first stable release. The rc.1
   attempt exposed a draft lookup bug before publication; rc.2 is the first
   intended installable private release.
2. Update Cargo.toml and the root package version in Cargo.lock. Add exactly one
   nonempty `## [VERSION]` section to CHANGELOG.md. Describe supported Codex
   versions, state/configuration compatibility, and recovery steps when relevant.
3. Run `nix develop --command pkf run check` and review the PR archive smoke job.
   Do not merge the version change until all checks succeed: merging it publishes
   automatically. Keep normal feature PRs separate from release version changes.
4. After merge, inspect the Release workflow and install the published archive
   on a clean supported machine. The workflow tags the tested commit, uploads all
   assets to a draft, checks the asset names, then publishes it. RC versions are
   prereleases and do not become the latest stable release.

The initial implementation PR contains the first RC version change, so merging
it is the publication decision. It does not need a previously created tag or a
personal access token: the publishing job alone receives `contents: write`.
Other jobs use read-only tokens. The workflow does not depend on tag events
triggering another workflow, avoiding GITHUB_TOKEN event-chaining limitations.

Repository settings must allow GitHub Actions and the pinned first-party actions.
Protect main and require `Development checks` and the Release `build` job before
merging. Any rules restricting tag creation must permit the release workflow.
No Apple Developer signing/notarization credentials are used at this stage.

Draft discovery uses the authenticated paginated release list, not the by-tag
endpoint, which excludes unpublished drafts. Once discovered, the numeric
release ID is used to verify uploaded assets before announcement. Both draft
discovery and same-commit draft recovery have focused regression tests.
New drafts retain the ID returned by the creation API itself: release lists can
be stale immediately after creation. The rc.2 publication completed on a retry
using its existing draft; retaining the creation response removes that initial
read-after-write dependency for subsequent releases.

The repository was private when this pipeline was prepared. Releases inherit
that visibility: public distribution requires the owner to review the complete
repository and history before choosing to make it public, or to design a
separate public distribution repository. Do not change repository visibility as
part of a release workflow. Private releases can be tested by authorized users.

## Dry runs, failures, and retries

Run the Release workflow manually on main with `publish` left false for a dry
run. Download its `tested-release` Actions artifact to inspect the tested
archive and license bundle. PR artifacts are never published as GitHub Releases.

For a failed release, first inspect the failed step. A pre-publication failure
must be fixed before retrying; do not manually publish the draft. Re-run failed
jobs on the same workflow commit to reuse that commit and its tested artifact.
A manual run with `publish: true` on main is also available for an intentional
release retry, but it builds the current main version and does not override an
already published version. A same-name tag pointing to another commit fails.

A failed upload can leave a tag and a draft. A retry for the same commit can
replace assets in that draft, but unexpected leftover asset names block public
announcement for inspection. Never move a released tag or replace public
assets. Fixes after publication use a new version. Release runs are serialized
and are not canceled when a newer release is queued.

## Local distribution verification

```sh
bash scripts/install_dist.sh
pkf run release-build
```

The installer downloads only the packaging tool into `target/release-tools/`.
`DIST_BIN=/absolute/path/to/dist` can select an already downloaded 0.33.0 binary.
The local build writes `target/release-plan/` and never publishes. Rust must be
the version in rust-toolchain.toml; a rustup installation also needs `rust-docs`
for library attributions. Use the official rustup toolchain outside `nix develop`
and ensure `cargo` and `rustc` resolve to that toolchain. The helper rejects a
Nix compiler because its runtime dependencies can refer to the Nix store.
Python 3.12+ and Xcode Command Line Tools are also required to build/test locally.
Native builds set `MACOSX_DEPLOYMENT_TARGET=15.0`.
The smoke check rejects Nix/Homebrew/custom dynamic-library dependencies.

`release_notices.py` conservatively retains all license alternatives and nested
notices for the target metadata graph, including build tools and procedural
macros. An unreviewed license expression, missing text, or changed SDK supplement
pin fails packaging. The inventory records versions, lock hash, notice hashes,
and the compiler. Rust library copyright and license texts are bundled too.
Dependency updates still need review for per-file exceptions: automated license
metadata checks do not certify all source content. See [license review](license-review.md).

## Next stage: Homebrew and release PR automation

After the first RC passes end-user installation checks:

1. Create `beso1225/homebrew-tap` and choose how to authorize updates to that
   repository. Prefer a GitHub App scoped to that tap, or a fine-grained token
   with tap contents write permission, stored as an Actions secret. The default
   GITHUB_TOKEN cannot update a separate repository.
2. Add a formula generated from a published stable release. It must restrict the
   supported OS/architecture, retain both original licenses and third-party
   notices, and test `--version`. Test install, upgrade, and uninstall on a
   clean machine. Keep explicit LaunchAgent configuration until its state/config
   handling is represented correctly; do not start a default Worker on install.
3. Publish formula updates only after the referenced GitHub Release is public.
   Keep RC versions out of the normal stable formula.
4. Add release PR generation after this flow is proven. Its sole job is to
   propose version, lock file, and changelog changes. The main version-change
   trigger remains responsible for publishing after the PR is reviewed/merged.

Homebrew publishing, automatic release PR creation, additional OS targets,
Apple Developer ID signing/notarization, and a Nix package are not enabled by
the initial pipeline. No tap credential is required for the first RC.
