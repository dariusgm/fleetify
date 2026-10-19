#!/bin/sh
# Verifier: the lock matches Cargo.toml, the project builds with all targets
# and the tests pass.
set -eu
repo="$1"
cd "$repo"

cargo metadata --locked --format-version 1 >/dev/null
cargo build --locked --all-targets
cargo test --locked
echo "ok: build and tests pass with the updated dependencies"
