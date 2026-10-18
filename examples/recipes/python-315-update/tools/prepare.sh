#!/bin/sh
# Prepare step: bump the Python version in the three files that carry it.
# Exits non-zero when a file does not contain what the recipe expects.
set -eu
repo="$1"
cd "$repo"

grep -q '^FROM python:3\.11\.8 AS base$' Dockerfile || { echo "E  Dockerfile does not start from python:3.11.8"; exit 1; }
sed -i 's/^FROM python:3\.11\.8 AS base$/FROM python:3.15.0 AS base/' Dockerfile

grep -q '^python = "3\.11\.\*"$' pyproject.toml || { echo "E  pyproject.toml does not pin python 3.11.*"; exit 1; }
sed -i 's/^python = "3\.11\.\*"$/python = "3.15.*"/' pyproject.toml

# Regenerate the lock for the new constraint. Plain `poetry lock` keeps every
# existing pin; only the header, python-versions and content-hash change.
uvx --from poetry==2.5.1 poetry lock
