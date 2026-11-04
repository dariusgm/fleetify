#!/bin/sh
# Verifier: the same checks as the project's CI, run on the updated tree.
set -eu
repo="$1"
cd "$repo"

cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo build --verbose
cargo test --all-targets --verbose

if [ -f pyproject.toml ]; then
  # The Python tests load the extension built above, packaged with DuckDB's tools.
  # The checkout lives outside the workspace, so it never reaches a commit.
  cache="${XDG_CACHE_HOME:-$HOME/.cache}/fleetify/extension-ci-tools"
  if [ ! -d "$cache" ]; then
    git clone --depth 1 https://github.com/duckdb/extension-ci-tools.git "$cache"
  fi
  export GIT2DUCK_PACKAGER="$cache/scripts/append_extension_metadata.py"
  uv sync
  uv run pytest
fi
echo "ok: Cargo and Python checks passed"
