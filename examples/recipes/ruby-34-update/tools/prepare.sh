#!/bin/sh
# Usage: prepare.sh <repo-dir>
# Sets the Ruby pins to 3.4. Runs in the workspace.
set -eu

cd "$1"

printf '3.4\n' > .ruby-version
sed -i 's/^image: ruby:.*/image: ruby:3.4/' .gitlab-ci.yml
