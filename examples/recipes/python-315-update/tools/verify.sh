#!/bin/sh
# Verifier: the pins are right, the lock matches pyproject.toml, the project
# installs under Python 3.15 and the site builds with Pelican.
set -eu
repo="$1"
cd "$repo"
poetry="uvx --from poetry==2.5.1 poetry"

grep -q '^FROM python:3\.15\.0 AS base$' Dockerfile || { echo "E  Dockerfile is not on python:3.15.0"; exit 1; }
grep -q '^python = "3\.15\.\*"$' pyproject.toml || { echo "E  pyproject.toml is not 3.15.*"; exit 1; }

$poetry check --lock

python315="$(uv python find 3.15)"
export POETRY_VIRTUALENVS_IN_PROJECT=false
$poetry env use "$python315"
$poetry install --no-root --no-interaction
$poetry run python -c 'import sys; assert sys.version_info[:2] == (3, 15), sys.version; print("ok: python", sys.version.split()[0])'

$poetry run make html
echo "ok: site built with Pelican under Python 3.15"
