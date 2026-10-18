#!/usr/bin/env bash
# Fast verifier: the pipeline file must exist. Exit 1 when it does not.
set -eu
cd "$1"
if [ ! -f .gitlab-ci.yml ]; then
  echo "E  .gitlab-ci.yml is missing"
  exit 1
fi
echo "ok: .gitlab-ci.yml present"
