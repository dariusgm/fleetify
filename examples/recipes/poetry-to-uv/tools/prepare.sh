#!/bin/sh
# Prepare: poetry.lock is replaced by uv.lock, so the old lock goes first.
set -eu
repo="$1"
cd "$repo"
if [ -f poetry.lock ]; then
  rm -f poetry.lock
fi
