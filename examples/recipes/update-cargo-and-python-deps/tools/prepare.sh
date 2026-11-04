#!/bin/sh
# Prepare step: bring every dependency to the newest version its constraints allow,
# and to the newest major version for Cargo.
set -eu
repo="$1"
cd "$repo"

if [ -f Cargo.toml ]; then
  # cargo-edit provides `cargo upgrade`, which can cross major versions.
  cargo upgrade --version >/dev/null 2>&1 || cargo install cargo-edit --locked
  cargo upgrade --incompatible
  cargo update

  # gix 0.89 changed the tree-diff callback: it returns a ControlFlow and uses
  # gix::Error. The substitution is idempotent, so it is a no-op once applied.
  sed -i 's/Ok::<_, anyhow::Error>(gix::object::tree::diff::Action::Continue)/Ok::<_, gix::Error>(std::ops::ControlFlow::<()>::Continue(()))/' src/lib.rs
fi

if [ -f pyproject.toml ]; then
  # Re-resolves every package in uv.lock within the constraints in pyproject.toml.
  uv lock --upgrade
fi
