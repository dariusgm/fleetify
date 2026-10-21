# Manual end-to-end check

`run_e2e.py` runs fleetify against real hosts and real agents, with small throwaway repositories. It
is a manual step after a larger change. It is not part of CI.

## What it covers

| Case | Host | Agent | Expected result |
|------|------|-------|-----------------|
| plain (no agent, prepare + verify) | GitHub, GitLab | none | `pr_open` |
| verify fails | GitHub, GitLab | none | `verify_failed` |
| agent changes the Ruby pin | GitHub, GitLab | selected agents | `pr_open` |
| agent + wait for CI | GitHub (Actions), GitLab (CI) | selected agents | `ci_passed` |
| agent + wait for CI + merge when green | GitHub, GitLab | selected agents | `merged` |

Agents: `opencode-llamacpp` (default, local llama.cpp server), `copilot`, `claude-code`. Copilot and
Claude Code use their own quota and are only run when named.

## Requirements

- A built fleetify binary (`cargo build`), or `--fleetify PATH`.
- `gh` and `glab` logged in as the owner account, and `git`.
- `~/.fleetify/credentials.toml` with the owner's tokens. fleetify uses these to push and open
  merge/pull requests; the script copies the file into its work folder.
- For `opencode-llamacpp`: the llama.cpp server reachable (`FLEETIFY_E2E_LLAMA_URL`, default
  `http://192.168.178.151:8080/v1/models`) and the `llamacpp` provider in the opencode config.

### Token permissions

Both the CLI login (`gh`, `glab`, used to create and delete repositories) and the fleetify
credential (used to push, open PRs and read CI) are checked before anything is created.

| Host | Needed for | Scopes |
|------|------------|--------|
| GitHub | create private repos, push, pull requests, Actions runs, merge | `repo`, `workflow` (pushing `.github/workflows/` needs `workflow`) |
| GitHub (gh login only) | delete the run's repos at the end (`--cleanup --delete`, or the automatic delete on success) | additionally `delete_repo` |
| GitLab | create projects, push, merge requests, pipelines, delete projects | `api` |

Classic GitHub tokens show their scopes; a fine-grained token shows none, and the check then asks you to
verify it by hand. Check the current tokens without creating anything:

    python3 scripts/e2e/run_e2e.py --check-tokens            # create and run needs
    python3 scripts/e2e/run_e2e.py --check-tokens --delete   # plus delete

To add the delete scope to the GitHub login: `gh auth refresh -s delete_repo` (interactive, run it
yourself with `!` in the prompt). Only the `gh` login needs it: the script creates and deletes
repositories through `gh`; fleetify's own credential does not need `delete_repo`.

## Usage

    python3 scripts/e2e/run_e2e.py --plan                          # print the plan, create nothing
    python3 scripts/e2e/run_e2e.py                                 # opencode-llamacpp on both hosts
    python3 scripts/e2e/run_e2e.py --agents opencode-llamacpp,copilot --hosts github
    python3 scripts/e2e/run_e2e.py --cleanup <run-id>              # list a run's repositories
    python3 scripts/e2e/run_e2e.py --cleanup <run-id> --delete    # delete them

Safety:

- Repositories are private by default. `--visibility public` makes them public on both hosts. Public
  repos show PRs, agent output and CI logs to everyone while they exist; the fixtures contain no secrets.
  Whether GitLab gives public projects more free CI minutes is not checked here.
- Every repository is named `fleetify-e2e-<run>-<case>`. The script refuses a plan of more
  than 14 repositories.
- It asks before creating anything, unless `--yes` is given.
- Results are compared with the expected results above; the summary is written to
  `<workdir>/e2e-summary.md`.
- Timings per case (repository setup, fleetify run, total) are in the summary and in
  `<workdir>/timings.json`. Reference numbers from earlier runs are in `REFERENCE.md`.

After the run:

- All cases match: the repositories are deleted again (if the tokens have the delete scope; otherwise
  they are listed for manual removal).
- Any mismatch: nothing is deleted. The repositories and the work folder (run folders, logs, reports) are
  kept, so the failure can be read. Remove them later with `--cleanup <run-id> --delete`.
- `--keep`: keep the repositories in every case.
