#!/bin/sh
# Usage: verify.sh <repo-dir>
# Checks that the Ruby pins agree on 3.4.
set -eu

cd "$1"

version=$(cat .ruby-version)
[ "$version" = "3.4" ] || { echo "error: .ruby-version is $version, expected 3.4"; exit 1; }
grep -q '^image: ruby:3\.4$' .gitlab-ci.yml || { echo "error: .gitlab-ci.yml does not use ruby:3.4"; exit 1; }
echo "ok: Ruby 3.4 pinned"
