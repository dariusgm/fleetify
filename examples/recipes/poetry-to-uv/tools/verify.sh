#!/bin/sh
# Verifier: uv.lock matches pyproject.toml, the lower bounds hold, the environment
# installs from the lock, and the site builds with uv.
set -eu
tools="$(cd "$(dirname "$0")" && pwd)"
repo="$1"
cd "$repo"

uv lock --check
python3 "$tools/check_lower_bounds.py" "$repo"
uv sync --frozen
uv run make html
echo "ok: uv.lock is current, lower bounds hold and the site builds with uv"
