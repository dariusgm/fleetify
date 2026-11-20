#!/usr/bin/env bash
# Verifier for posix: the change must not add whitespace errors. Exit 1 when it does.
set -eu
cd "$1"
if ! git diff --check; then
  echo "E  whitespace errors in the change"
  exit 1
fi
echo "ok: no whitespace errors"
