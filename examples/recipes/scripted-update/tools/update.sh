#!/bin/bash
# The change: re-resolve the uv lock within the constraints in pyproject.toml.
# Runs with bash; the argument is the repository path.
set -euo pipefail
repo="$1"
cd "$repo"
uv lock --upgrade
