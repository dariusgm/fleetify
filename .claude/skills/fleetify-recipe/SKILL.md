---
name: fleetify-recipe
description: Turn a list of Git repositories and a prompt into a fleetify recipe, validate it, dry-run it, and run it with Claude Code as the agent. Use when the user wants one change applied across several repositories with fleetify.
---

# fleetify recipe

Builds a fleetify recipe from a repository list and a prompt, checks it, dry-runs it, and runs it with `claude-code` as the agent. Each repository gets its own branch and pull request. Nothing is merged.

## Inputs

Ask only for what is missing.

- **Repositories**, one per line. Accepted forms: `github:owner/name`, `gitlab:group/name`, or a full `https://` or `git@` URL. A bare `owner/name` is refused, because the host would be ambiguous.
- **Prompt**. Keep it verbatim.
- **Name**. Lowercase letters, digits and dashes. Default: derived from the prompt, e.g. `cargo-deps-update`.
- **Branch** and **PR title**. Defaults: `chore/<name>` and a short title taken from the prompt.

Fixed choices, to state in the reply: agent `claude-code`; the verifier is the cargo verifier below; CI is awaited; merge is off.

## Where the recipe goes

`~/.fleetify/recipes/<name>/`. Keep it out of the fleetify repository.

```
recipe.toml
prompts/task.md
tools/verify.sh
repos.txt
```

## recipe.toml

This template is for Rust projects. For another language, replace `has_file`, `needs`, the verifier and the guards.

```toml
schema = 1
name = "<name>"
branch = "<branch>"
pr_title = "<pr title>"
commit_message = "<pr title>"
agent = "claude-code"
tools = "tools"

[applies_to]
host_type = ["github", "gitlab"]
has_file = ["Cargo.toml", "Cargo.lock"]

[[instructions]]
file = "prompts/task.md"

[needs]
commands = ["cargo"]

[verify]
argv = ["sh", "{tools}/verify.sh", "{repo}"]
timeout_s = 3600
tail_lines = 60
error_patterns = ['^error', '^E ', 'panicked', 'FAILED']

[guards]
# Manifest, lock and Rust sources may change. An incompatible upgrade often needs
# source edits, so this is wider than the manifest-only cargo-deps-update example.
allow_paths = ["Cargo.toml", "Cargo.lock", "src/*", "tests/*", "examples/*", "benches/*", "build.rs"]
max_files_changed = 50
max_lines_changed = 2000
deny_added_patterns = ['git\s*=\s*"', 'path\s*=\s*"', "NOSONAR"]

[ci]
provider = "auto"
wait = false
timeout_min = 60
merge = "off"

[limits]
agent_timeout_min = 90
```

`has_file` needs both files, so libraries without a lock file are skipped, not failed.

## prompts/task.md

The user's prompt, verbatim, then these rules:

```
Rules:
- Find the newest published version of each dependency with `cargo info <crate>`.
- Raise the version requirements in Cargo.toml, including semver-incompatible ones, then run `cargo update`.
- Change source files only where a new version requires it, and keep the behaviour the same.
- Do not add suppression attributes, skip or disable tests, or add NOSONAR markers.
- Do not switch any dependency to a git or path source.
- If the build or tests cannot be made to pass, write blocked.md in the repository root explaining why. Do not guess.
```

## tools/verify.sh

```sh
#!/bin/sh
# Verifier: the lock matches Cargo.toml, the project builds with all targets
# and the tests pass.
set -eu
repo="$1"
cd "$repo"

cargo metadata --locked --format-version 1 >/dev/null
cargo build --locked --all-targets
cargo test --locked
echo "ok: build and tests pass with the updated dependencies"
```

The recipe runs it with `sh`, so it does not need the executable bit.

## repos.txt

One repository per line. Example:

```
github:dariusgm/kipper
github:dariusgm/templator
github:dariusgm/restore
github:dariusgm/glowing-happiness
```

## Permissions

The agent may run `cargo` only when both sides grant it. The recipe's `[needs]` asks for it. The user's `~/.fleetify/config.toml` must grant it:

```toml
[permissions]
commands = ["cargo"]
```

If the file is missing or lacks this entry, add it and tell the user. Without it, the run is refused before any repository is touched.

## Steps

1. Create the folder and write the four files. Fill the placeholders.
2. Validate: `fleetify validate ~/.fleetify/recipes/<name>`. It must print `recipe <name> is valid`. Exit code 3 means refused. Fix the listed fields and repeat.
3. Dry run: `fleetify run ~/.fleetify/recipes/<name> ~/.fleetify/recipes/<name>/repos.txt --dry-run`. Repositories without `Cargo.toml` or `Cargo.lock` should show as `skipped`. Report the planned table to the user.
4. Ask the user to confirm the real run. It pushes a branch and opens one pull request per repository. Only after a yes, run the same command without `--dry-run`.
5. Follow the run: `fleetify status --run <id> --watch`, then `fleetify report --run <id>`. Use `--failed` for only the failures. For a single repository, use `fleetify logs <repo> --run <id>`.

The run id defaults to `<name>-<YYYY-MM-DD>`. Pass `--run` to choose another.

## Outcomes

| Result | Meaning |
|---|---|
| `pr_open` | Pushed and opened a pull request. |
| `already_done` | An open pull request for the branch already exists. |
| `skipped` | `applies_to` is not met, e.g. no lock file. |
| `no_changes` | The agent changed nothing. |
| `needs_input` | The agent wrote `blocked.md`. Read the reason in the report. |
| `verify_failed` | The build or tests fail with the new versions. Nothing is pushed. |
| `guard_violation` | A changed file or pattern broke a guard. Nothing is pushed. |
| `base_broken` | The untouched base already fails the verifier. The repository is not changed. |
| `error` | Prepare, agent or host step failed. The report names the step. |
