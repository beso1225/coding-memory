#!/usr/bin/env bash
# Download the fixed packaging tool into this checkout, without global installs.
set -euo pipefail
cd "$(dirname "$0")/.."
test "$(uname -s)" = Darwin
test "$(uname -m)" = arm64
release_tool_tmp=$(mktemp -d)
trap 'rm -rf "$release_tool_tmp"' EXIT
curl --fail --location --retry 3 \
  https://github.com/axodotdev/cargo-dist/releases/download/v0.33.0/cargo-dist-aarch64-apple-darwin.tar.xz \
  --output "$release_tool_tmp/dist.tar.xz"
printf '%s  %s\n' \
  7b3cbe25511de01d74c0f5fcb7909edabd379bea9cfa284d93af5a3cdfa3247c \
  "$release_tool_tmp/dist.tar.xz" | shasum -a 256 -c -
tar -xf "$release_tool_tmp/dist.tar.xz" -C "$release_tool_tmp"
mkdir -p target/release-tools
cp "$release_tool_tmp/cargo-dist-aarch64-apple-darwin/dist" target/release-tools/dist
target/release-tools/dist --version
