#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export MACOSX_DEPLOYMENT_TARGET=15.0
case "$(rustc --print sysroot)" in
  /nix/*)
    echo 'Distribution builds require an official rustup toolchain outside nix develop; see docs/releasing.md.' >&2
    exit 1
    ;;
esac
release_dist=${DIST_BIN:-target/release-tools/dist}
test "$("$release_dist" --version)" = 'cargo-dist 0.33.0'
release_version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["package"]["version"])')
python3 scripts/release.py plan
cargo fetch --locked --target aarch64-apple-darwin
python3 scripts/release_notices.py
"$release_dist" generate --check
"$release_dist" build --artifacts=local --target=aarch64-apple-darwin \
  --tag="v$release_version" --no-local-paths --output-format=json > target/release-dist-manifest.json
release_archive=target/distrib/coding-memory-aarch64-apple-darwin.tar.gz
python3 scripts/release_smoke.py "$release_archive" --version "$release_version"
mkdir -p target/release-plan/artifacts
cp "$release_archive" target/release-plan/artifacts/
cp "$release_archive.sha256" target/release-plan/artifacts/
cp target/release-dist-manifest.json target/release-plan/artifacts/dist-manifest.json
# Use a standard filename and relative archive path for user-side verification.
(cd target/release-plan/artifacts && shasum -a 256 coding-memory-aarch64-apple-darwin.tar.gz > SHA256SUMS)
