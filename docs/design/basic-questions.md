# The four basic questions

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

**1. Where does the list of repos come from?**
A text file `repos.txt`, one repo per line, `#` for comments:

```
# GitHub
github:myname/project-a
git@github.com:myname/project-b.git

# GitLab (gitlab.com, subgroups allowed)
gitlab:mygroup/subgroup/project-c

# self-hosted GitLab: full URL, host must be declared in the recipe
https://gitlab.example.com/team/project-d.git
```

Accepted forms: `github:owner/name` and `gitlab:group/.../name` (github.com / gitlab.com); a full clone URL (HTTPS or SSH). `github.com` and `gitlab.com` are recognised. The host comes from the URL. Every other host must be declared in the recipe (`[hosts."<name>"] type = "github"|"gitlab"`), nothing is guessed. A bare `owner/name` is rejected because the host would be ambiguous. Every line normalises to a `Repo` with `host`, `host_type`, `path` (full path including subgroups), `name` (last path segment) and a clone URL. The file lives in Git and is reviewable. Generate it with a small script against the host API if needed, then review by hand.

**2. Where do the repos live?**
fleetify owns a working directory (default `~/.fleetify/`, configurable) and never touches your normal checkouts.

```
~/.fleetify/clones/<host>/<path>/                 # one cache clone per repo, reused, always on the default branch
~/.fleetify/runs/<run-id>/report.json             # the run's report, rewritten at the end of every run (see "Report")
~/.fleetify/runs/<run-id>/results.jsonl           # append-only, one line per repo per attempt; the last line per repo counts
~/.fleetify/runs/<run-id>/fleetify.log            # the tool's own trace log (JSON lines)
~/.fleetify/runs/<run-id>/<host>/<path>/          # workspace + logs + state for one repo
```

**Run id:** `<recipe>-<YYYY-MM-DD>` by default, overridable with `--run NAME`. Starting the same recipe on the same day reuses the run folder and resumes (see "Run flow per repo"); a new day starts a new run folder, and idempotency is then enforced through the host (existing branch or PR/MR), not through the state files.

**The workspace is a self-contained local clone, not a `git worktree`.** A worktree's `.git` points back into the cache clone outside the workspace, which a workspace-confined agent may not be allowed to read (breaking `git status` and `git diff`). So each run gets a `git clone --local` from the cache (hard links on the same filesystem, otherwise a copy; verify the behaviour on Windows/NTFS), with `origin` pointed at the real remote and `remote.origin.pushurl` set to an invalid value. Everything the agent needs is inside one directory.

Folders mirror host and full path (including GitLab subgroups) so equal names never collide. First run clones, later runs fetch. Existing local checkouts are not supported in stage 1. **Windows:** keep the working directory path short or enable long paths, otherwise deep trees (e.g. `node_modules`) can hit the 260-character limit.

**3. Which repos are touched?**
A repo is processed when the recipe's `applies_to` conditions are true. They are evaluated on the cache clone at the default branch, before any workspace is created. All others end as `skipped`. `--only` selects repos by path suffix (e.g. `project-a,subgroup/project-c`).

**4. What should be done?**
That is the recipe: a directory with `recipe.toml`, optional prompt and reference files, and a `tools/` directory with scripts.

```
recipes/enable-gitlab-ci/
  recipe.toml
  prompts/enable-ci.md
  reference/gitlab-ci.yaml
  tools/verify.sh
  tools/verify.ps1
```

```toml
# recipes/enable-gitlab-ci/recipe.toml
schema = 1
name = "enable-gitlab-ci"
branch = "chore/enable-gitlab-ci"
pr_title = "chore: enable GitLab CI pipeline"
agent = "claude-code"            # claude-code | opencode | copilot (or --agent); required iff instructions is non-empty

[applies_to]                     # all listed conditions must hold (AND); each list entry is OR within itself
host_type = ["gitlab"]
not_has_file = [".gitlab-ci.yml"]
# has_file = ["pyproject.toml"], path_glob = ["team-a/*"], has_file_any = ["package.json", "pnpm-lock.yaml"]

[[instructions]]                 # concatenated in order; empty = no agent, pure prepare
file = "prompts/enable-ci.md"

[[instructions]]
text = "The reference pipeline is in .agent/attachments/gitlab-ci.yaml. Adapt it to this repository and write .gitlab-ci.yml."

attachments = ["reference/gitlab-ci.yaml"]   # copied to .agent/attachments/
tools = "tools"                              # resolved by {tools}

[prepare]                        # optional deterministic step, runs in the workspace before the agent
posix = ["bash", "{tools}/prepare.sh", "{repo}"]
windows = ["pwsh", "-NoProfile", "-File", "{tools}/prepare.ps1", "{repo}"]
timeout_s = 120

[needs]                          # declared needs beyond the baseline; checked against credentials.toml, never grants
commands = ["uv run pytest"]
web = false
extra_directories = []

[verify]
posix = ["bash", "{tools}/verify.sh", "{repo}"]
windows = ["pwsh", "-NoProfile", "-File", "{tools}/verify.ps1", "{repo}"]
timeout_s = 300
tail_lines = 40                  # lines shown to the agent on failure
error_patterns = ['^E\s', 'error:']   # regexes that select the relevant lines on failure

[guards]
deny_paths = ["sonar-project.properties"]
deny_added_patterns = ["NOSONAR", '#\s*nosec', '#\s*noqa']
max_files_changed = 50
max_lines_changed = 2000
forbid_test_deletion = true
forbid_binary_files = true
# allow_paths = ["**/*.py", "pyproject.toml"]

# [judge]                        # optional
# agent = "opencode"
# profile = "judge"
# max_diff_lines = 1500

[ci]
provider = "auto"                # auto | gitlab-ci
wait = true
timeout_min = 30
merge = "off"                    # off | when_green

[limits]
agent_timeout_min = 60
idle_warn_min = 10
```

- All paths in a recipe (`file`, `attachments`, `tools`) are relative to the recipe directory. A command table has `posix` and/or `windows` argument lists; a command with a single `argv` list applies to both.
- `{repo.name}`, `{repo.path}`, `{repo.default_branch}` are substituted in instruction text. A recipe with no `instructions` runs no agent (pure `prepare`, e.g. Dependabot); `agent` is then ignored. Validation fails if `instructions` is non-empty and no agent is given by recipe or `--agent`, and if a branch name, commit message or PR title contains a skip-CI marker.
- `attachments` are copied into the workspace under `.agent/attachments/`. `.agent/` is listed in `.git/info/exclude`, so reference files are never committed.
- `tools` is the directory holding verifier and prepare scripts. It is copied into the workspace as `.agent/tools/` for the agent (the untrusted copy) and used from its original location by the orchestrator (the trusted copy). `{tools}` resolves to whichever applies.
- `fleetify schema` prints the JSON Schema of `recipe.toml` for the current binary; `fleetify validate recipes/x/` checks a recipe against it, resolves every referenced file and prints the rendered commands for both OS. A hand-written and a generated recipe are treated exactly the same.

**Python authoring layer (optional, separate package `fleetify-recipes`).** Dataclasses that mirror the TOML schema with type hints and docstrings, `Recipe(...).write("recipes/x/recipe.toml")`, and a validation against the JSON Schema exported by the binary, so IDE completion and early errors are available while writing recipes. It is a build-time aid: the binary never imports or runs it, and nothing in it is needed to run a recipe. Its schema version is pinned to the binary's `schema` number; a mismatch is an error, not a warning.

