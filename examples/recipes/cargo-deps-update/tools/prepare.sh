#!/bin/sh
# Prepare step: raise the version requirements in Cargo.toml to the newest
# releases, then refresh Cargo.lock. Exits non-zero when Cargo.toml is missing.
set -eu
repo="$1"
cd "$repo"

[ -f Cargo.toml ] || { echo "E  Cargo.toml not found"; exit 1; }

# cargo-edit provides `cargo upgrade`, which can cross semver-incompatible
# versions. Install it once when it is missing.
if ! cargo upgrade --version >/dev/null 2>&1; then
  cargo install cargo-edit --locked
fi

cargo upgrade --incompatible
cargo update
