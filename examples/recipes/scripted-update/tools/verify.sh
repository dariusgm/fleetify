#!/bin/bash
# Verifier: the lock must match pyproject.toml, and the tests must pass.
set -euo pipefail
repo="$1"
cd "$repo"
uv lock --check
uv run pytest -q
echo "ok: lock and tests"
