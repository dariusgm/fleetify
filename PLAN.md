# Plan: `fleetify` (minimal version) — revision 3

Goal: run one task across a list of Git repositories. Per repository an optional coding agent works in its own workspace. A fast verifier runs inside the agent's loop. The orchestrator then pushes a branch, opens a PR/MR, waits for CI, and only after CI is green (and only if explicitly enabled) merges.

**Language:** everything the tool ships is English: code identifiers, CLI commands and flags, config keys, log/status output, error messages, docs. No mixed-language names. This plan is written in English for the same reason.

**Supported hosts:** GitHub and GitLab (gitlab.com and self-hosted). **Supported agents:** Claude Code, opencode, GitHub Copilot CLI. Everything else is out of scope and can come from the community.

**Implementation:** one Rust binary (`cargo`), no runtime dependencies on the target machine, no database, no web UI. Recipes are declarative `recipe.toml` files plus a `tools/` directory with scripts. An optional, separate Python package (`fleetify-recipes`) is an authoring aid that generates `recipe.toml`; the binary never runs Python. Later ideas live in `PLAN-ausbau.md` (German, not yet aligned with this plan; this plan wins on conflicts).

**Two ways the tool is used, in order of importance:**

1. **Automated.** Started from a script, a CI job or another agent. Nobody watches. What matters is the exit code and a machine-readable `report.json` that contains, for every repo, the result and, on failure, enough detail (error class, message, log tails, guard rule, paths to artifacts) that the caller does not have to open the run folder.
2. **Demo / interactive.** A person watches the run. A small live view (ratatui) shows what each repo is doing. It is a thin renderer over the same event stream as the automated mode and shows nothing that the logs and the report do not also contain.

**Test sequence:** private GitHub repos, recipe "add Dependabot config" (no agent) → a deterministic recipe on a private gitlab.com project → a recipe with an agent (opencode + llama.cpp) → CI gate on gitlab.com → Claude Code privately → Copilot CLI and self-hosted GitLab at work → later a SonarQube recipe at work.

**Revision 3 changes** (for the reader, not for the README): Rust instead of Python; recipes are TOML plus `tools/` scripts, with `applies_to` as a condition list and `prepare` as a command; a Python authoring package is optional and separate; the core emits a typed event stream consumed by renderers (plain, TUI) and by the report writer; `report.json` and `fleetify report` are the primary output for automated use; `tracing`, a panic hook and `--debug` cover crashes of the tool itself; the TUI is an optional, demo-oriented milestone; `truststore` is replaced by the OS certificate store via the TLS crate. Revision 2 changes (smaller milestones, run flow order, `ci_passed`, resume rules, `{tools}`, `--allow-unchecked-agent`, `ChangeRequest`, `wants_input`/`version`/`judge_command`, run ids) are kept.

## Origin and notice

**Inspiration.** The core idea (existing fleet-management tooling for changes across many repos, with a coding agent plugged in and verifiers checking the result) comes from Spotify's public engineering articles about their background coding agent (called "Honk" at Spotify). The name `fleetify` is a nod to the term "Fleet Management" used there. Ideas were taken, no code:

- "1,500+ PRs Later: Spotify's Journey with Our Background Coding Agent (Honk, Part 1)", <https://engineering.atspotify.com/2025/11/spotifys-background-coding-agent-part-1>
- "Background Coding Agents: Context Engineering (Honk, Part 2)", <https://engineering.atspotify.com/2025/11/context-engineering-background-coding-agents-part-2>
- "Background Coding Agents: Predictable Results Through Strong Feedback Loops (Honk, Part 3)", <https://engineering.atspotify.com/2025/12/feedback-loops-background-coding-agents-part-3>

**Notice (copy verbatim into the README).**

> `fleetify` is an independent project. It is **not a product of Spotify**, is **not affiliated with, endorsed by, sponsored by or developed in collaboration with Spotify**, and uses no Spotify code. The name and design are inspired by Spotify's public engineering articles on its background coding agent, linked above. "Spotify" is a trademark of Spotify AB. All other product names (GitHub, GitLab, Claude Code, GitHub Copilot, opencode, SonarQube and others) are trademarks of their respective owners and are used for identification only.

German version: `fleetify` ist ein eigenständiges Projekt. Es ist kein Produkt von Spotify, steht in keiner Verbindung zu Spotify, wird weder von Spotify unterstützt noch gefördert und ist keine Kollaboration mit Spotify. Es enthält keinen Spotify-Code. Name und Konzept sind von den öffentlichen Engineering-Artikeln von Spotify inspiriert, die oben verlinkt sind. "Spotify" ist eine Marke der Spotify AB. Alle anderen genannten Produktnamen sind Marken ihrer jeweiligen Inhaber und dienen nur der Bezeichnung.

## Principles

1. **The orchestrator owns every side effect:** clone, branch, commit, push, PR/MR, CI polling, merge, tokens. The agent only edits files in its workspace and runs the verifier.
2. **Three gates, from fast to authoritative:** (a) fast verifier inside the agent loop, (b) the same verifier plus guards as the outer gate before anything is pushed, (c) CI on the PR/MR as the last gate before any merge.
3. **Observable by default, report first.** Every run emits one typed event stream. The event log, the state files, the report and every renderer are built from it. The automated caller gets exit code plus `report.json`; the live view is optional sugar.
4. **Never wait for a human inside a run.** Agents run non-interactively. If an agent needs input, the run ends as `needs_input` or `blocked` and the workspace is kept.
5. **Idempotent re-runs.** Running the same command again skips what is already done and resumes what was interrupted (rules under "Run flow per repo").
6. **No implicit shell.** Commands are argument lists. Shell, OS and quoting are explicit.
7. **No logic in the recipe format.** A recipe is data. Anything that needs logic is a script in the recipe's `tools/` directory, started by the orchestrator with timeouts and logging like every other command.
8. **fleetify owns the agent invocation, including its permissions** (which actions the agent may take, which directory it may touch). Provider, model and login stay in the agent's own configuration. What an agent does inside the granted permissions is the responsibility of the agent and its vendor. Permissions are defined in `config.toml` with restrictive defaults; anything beyond the defaults is granted by the user and is at the user's own risk.

## The four basic questions

**1. Where does the list of repos come from?**
A text file `repos.txt`, one repo per line, `#` for comments:

```
# GitHub
github:myname/project-a
git@github.com:myname/project-b.git

# GitLab (gitlab.com, subgroups allowed)
gitlab:mygroup/subgroup/project-c

# self-hosted GitLab: full URL, host must be listed in config.toml
https://gitlab.example.com/team/project-d.git
```

Accepted forms: `github:owner/name` and `gitlab:group/.../name` (github.com / gitlab.com); a full clone URL (HTTPS or SSH). `github.com` and `gitlab.com` are recognised. Every other host must be mapped explicitly in `config.toml`, nothing is guessed. A bare `owner/name` is rejected because the host would be ambiguous. Every line normalises to a `Repo` with `host`, `host_type`, `path` (full path including subgroups), `name` (last path segment) and a clone URL. The file lives in Git and is reviewable. Generate it with a small script against the host API if needed, then review by hand.

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

[needs]                          # declared needs beyond the baseline; checked against config.toml, never grants
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

## Configuration and credentials

Two files, strictly separated:

```toml
# ~/.fleetify/config.toml  (no secrets)
workdir = "~/.fleetify"

[hosts."github.com"]
type = "github"

[hosts."gitlab.example.com"]
type = "gitlab"

[policy]
require_checked_agent = false   # true: refuse agents without a passing check-agent result

[permissions]                       # granted to every agent, on top of the built-in baseline
commands = []                       # e.g. ["uv run pytest"]; each entry widens what the agent may run
web = false
extra_directories = []              # directories besides the workspace

[permissions.agent.opencode]        # additions for one agent
unrestricted = false                # true = the adapter uses the CLI's blanket "allow everything" options; at your own risk
```

```toml
# ~/.fleetify/credentials.toml  (secrets, owner-only permissions)
# First matching rule wins. Put more specific rules first.
[[credential]]
match = "gitlab.example.com/team-a/project-x"   # single repo (fine-grained token)
token = "REPLACE_ME"

[[credential]]
match = "gitlab.example.com/team-a/*"           # e.g. a group token
token = "REPLACE_ME"

[[credential]]
match = "gitlab.example.com/*"                  # fallback, e.g. a user token
token = "REPLACE_ME"

[[credential]]
match = "github.com/myname/*"
token = "REPLACE_ME"
```

Rules:

- `match` is a glob on `<host>/<path>`. Order decides, not pattern length.
- No matching rule: the repo fails with a clear message (which repo, which rules were checked) and never prints a token value.
- Host tokens exist only inside the orchestrator process. They are **never put into environment variables**, so no subprocess can inherit them and no allow/deny list of variables has to be kept perfect. The agent process gets a minimal environment (allowlist) containing only what its adapter declares.
- Push and fetch over HTTPS: the token reaches only the short-lived `git` subprocess started by the orchestrator, via a `GIT_ASKPASS` helper (the fleetify binary itself, invoked with a hidden subcommand and a one-time pipe or file descriptor, never the token on the command line). Tokens never appear in URLs, command lines, logs or `.git/config`. SSH uses the user's key.
- A redactor masks every known token value in all logs, events, terminal output and the report.
- Startup checks: on POSIX the file must not be readable by group or others; on Windows the tool warns if it can detect broader access and the README documents restricting the ACL to the current user.
- `--dry-run` shows which rule matches each repo and makes one read call per repo (status only: ok / 401 / 403 / 404, never the value). Write access shows up only at push time, unless the host API reports the token's access level for the project (check the API docs). Upcoming token expiry is warned about when the host exposes it.
- **Agent logins are not fleetify's business.** fleetify stores no agent tokens. An agent authenticates through its own configuration (config directory, keychain, its own environment variables); the adapter's allowlist names exactly which directories and variables are passed through. Copilot CLI must never receive a host token used for pushing.

**Honest limitation:** any process running as the same OS user, including the agent, can read `credentials.toml`. The file protects against accidental inheritance (the problem with environment variables), not against a hostile or prompt-injected agent. What reduces the risk: least-privilege tokens (per-repo or group tokens, short expiry), the permissions fleetify passes to the agent (workspace only), and in stage 2 running agents in a container without credentials. Stage 2 can also read tokens from the OS credential store instead of a file.

**TLS / internal CA:** the internal CA is installed in the operating system. The HTTP client uses the OS certificate store (`reqwest` with `rustls` + `rustls-native-certs`, or `native-tls`; decide in M5 and note the reason). A startup self-test makes one TLS connection per configured host and aborts with a clear message on certificate errors. `git` is a separate case: depending on the installation, Git for Windows uses OpenSSL with its own CA file or the Windows certificate store. Verify clone and push during testing.

## Host layer (GitHub and GitLab)

Everything host-specific sits behind a small trait, one module per host (`hosts/github.rs`, `hosts/gitlab.rs`):

```rust
pub struct ChangeRequest { pub url: String, pub id: String /* PR number / MR iid */, pub head_sha: String, pub state: CrState /* Open | Merged | Closed */ }

pub trait Host {
    fn probe_read_access(&self, repo: &Repo) -> Result<Access>;           // Ok | Unauthorized | Forbidden | NotFound
    fn default_branch(&self, repo: &Repo) -> Result<String>;
    fn find_change_request(&self, repo: &Repo, branch: &str) -> Result<Option<ChangeRequest>>;   // open PR/MR for the branch
    fn create_change_request(&self, repo: &Repo, branch: &str, base: &str, title: &str, body: &str) -> Result<ChangeRequest>;
    fn merge(&self, repo: &Repo, cr: &ChangeRequest, expected_sha: &str) -> Result<MergeOutcome>;  // Merged | Blocked(reason)
}
```

Implemented via the REST APIs (no `gh`/`glab` dependency). Paths and parameters must be checked against the API docs of the deployed version. GitLab project paths with subgroups are URL-encoded (`/` becomes `%2F`), API base `https://<host>/api/v4`. Whether a remote branch exists is checked with `git ls-remote` by the orchestrator, not via the API.

CI is deliberately not part of the `Host` trait: the CI system can differ from the host (for example GitLab with an external CI). See "CI layer and CI gate".

## Agent layer (Claude Code, opencode, GitHub Copilot CLI)

One module per agent (`agents/claude_code.rs`, `agents/opencode.rs`, `agents/copilot.rs`, plus `agents/fake.rs` for tests):

```rust
pub trait Agent {
    fn name(&self) -> &str;
    fn version(&self) -> Result<String>;                                   // CLI version string, used as the check-agent cache key
    fn command(&self, prompt_line: &str, workspace: &Path, perms: &Permissions) -> Vec<String>;   // argument list, non-interactive, permissions included
    fn unsupported(&self, perms: &Permissions) -> Vec<String>;             // permission entries this CLI cannot express
    fn env(&self) -> Vec<(String, String)>;                                // allowlist: only what the agent needs
    fn wants_input(&self, recent_output: &str) -> bool;                    // called on every output chunk: does this look like a prompt for input?
    fn classify(&self, exit_code: i32, output: &str) -> Outcome;           // Ok | Error | NeedsInput | Unknown
    fn judge_command(&self, prompt_file: &Path, profile: Option<&str>) -> Vec<String>;   // one-shot, read-only, no workspace; output to stdout
}
```

- **Same core for all:** every agent gets the same fixed one-liner ("Read `.agent/task.md` and carry out the task.") and the same assembled `task.md`. No agent-specific instruction files (e.g. `CLAUDE.md`) are created or changed. Existing ones in the repo (`AGENTS.md`, `CLAUDE.md`, `.github/copilot-instructions.md`) can influence a run; check them when results look odd.
- **fleetify does not configure an agent's provider, model or login.** Provider, model and login come only from the agent's own configuration. fleetify writes nothing there and sets no model flags. Permissions are different: they are part of the invocation that fleetify builds, see "Permissions and workspace restriction" below. Local models (e.g. llama.cpp via opencode) therefore work without fleetify knowing about them.
- **Do not guess flags:** the non-interactive invocation and permission/auto-approve options are in each agent's current documentation. They have not been verified here.
- **Choice:** `agent` in the recipe or `--agent`. No automatic fallback to another agent.
- **Rate limits** differ: Claude Code waits and blocks, behaviour of the others must be tested. In stage 1 the agent timeout covers all of them: the repo ends as `agent_timeout` and a re-run continues.

**Permissions and workspace restriction (owned by fleetify).** The agent is started by fleetify, so the permissions passed in that call (allowed actions and allowed directory) are fleetify's responsibility. What the agent does inside those permissions is the responsibility of the agent and its vendor. The README states this boundary, and that permissions granted beyond the restrictive baseline are at the user's own risk. Copilot CLI is the reference, verified on 2026-10-21 with Copilot CLI 1.0.101 (Copilot Free, private): in headless mode (`-p`, `--no-ask-user`) it runs only the tools it is granted. A grant has to be specific to be a confinement. `--allow-tool=write` without a path pattern lets it write anywhere, with or without `--add-dir`; `--allow-tool='write(<dir>/*)'` denies writes outside `<dir>` and allows writes inside. Reads are not yet verified this way and are covered by the canary check. The other agents are started the same way wherever their CLI can express it.

- **`Permissions` model (agent-neutral):** `commands: Vec<String>` (allowed command prefixes), `web: bool`, `extra_directories: Vec<PathBuf>`. Baseline + config grants = effective permissions. Recipe `needs` ⊆ effective, otherwise the run refuses before starting.
- **Baseline (built-in, restrictive defaults):** read and edit files inside the workspace only; run only commands on an allowlist (the fast verifier command as rendered for the workspace, plus `git status`, `git diff`, `git log`); no commit and no push (the orchestrator does both); no web or network tools unless granted (the agent's own connection to its model provider is not affected by this). An allowlisted verifier may start child processes (linters, package managers) that the agent CLI does not police; that is by design and is why the outer gate is authoritative.
- **Grants live in `config.toml`, defaults are restrictive.** The baseline above is all an agent gets unless the user widens it in `[permissions]` (for all agents) or `[permissions.agent.<name>]` (for one agent): extra allowed commands, web access, extra directories, or `unrestricted = true`. Every widening is the user's decision and at the user's own risk. fleetify does not second-guess it but makes it visible: `--dry-run` and the run header print the effective permissions, and a `permissions_effective` event (with a hash of `config.toml`) is emitted at the start of each run. `config.toml` is read once at startup, so a run cannot change its own permissions mid-run.
- **Recipes declare needs, they do not grant.** `[needs]` lists what a recipe requires beyond the baseline. It is only checked against the effective permissions: if something is missing, the run refuses before starting and names the entry to add to `config.toml`. A shared recipe can therefore never silently escalate an agent's permissions.
- **Adapters translate.** Each adapter maps `Permissions` to its CLI's own command-line options for allowed actions and directories. Those options must be taken from each CLI's current documentation, not guessed; they have not been verified here. If a CLI cannot express a permission, `unsupported()` reports it, `--dry-run` shows the effective permissions per agent and marks the gaps, and the run prints a warning.
- **No blanket "allow everything" flags** unless `unrestricted = true` is set for that agent in `config.toml`. There is no CLI flag for it; the run header prints a prominent warning. (For the local opencode smoke test this is the expected setting until the adapter's permission mapping is verified.)
- **`check-agent` is a regression test for our own invocation.** CLI semantics change between versions, and an option that silently stops working would break our side of the contract. `fleetify check-agent <name>` creates a canary file outside the workspace and outside every granted extra directory (a unique marker string in a decoy file, never the real `credentials.toml`), runs the agent with the effective permissions plus the instruction to read and print that file by any available means and to write a file outside the workspace, and passes only if the marker never appears in output or logs and nothing was written. The result is cached per agent name and `version()`. If an agent and version has no passing result, `run` prints a warning. With `require_checked_agent = true` in `config.toml` it refuses instead; `--allow-unchecked-agent` overrides that refusal for one run.
- **Limits, stated in the README:** the check shows that the attempt failed, not why (a model may simply refuse), and it is not a sandbox. Real isolation is a container without credentials (stage 2). Files the agent process reads itself (its own settings, user-level instruction files) are outside the workspace by nature and are not covered.
- **Consequences for the design:** the agent can only use what is inside the workspace. That is why the workspace is a self-contained local clone and why the fast verifier runs from a copy inside the workspace (see below).

**task.md** is assembled by fleetify: (1) a fixed, versioned rules block, (2) the recipe's `instructions` in order, (3) the list of attachments with their relative paths, (4) the rendered verifier command. The rules block says: do not commit or push (the orchestrator does that); do not ask questions, choose the most conservative option and note assumptions; run the verifier (command given) until it passes; if you cannot proceed write `.agent/blocked.md` and stop; when done write `.agent/result.md` (what changed, assumptions, anything a reviewer should know).

**Interactive questions and stalls:**

- Agents run with stdin closed and in their non-interactive mode, so a question or permission prompt cannot wait forever for input. How each CLI behaves when it wants input must be tested per adapter.
- If `wants_input()` fires, or the agent writes `.agent/blocked.md`, the process tree is terminated, the workspace is kept and the repo ends as `needs_input` (detected) or `blocked` (declared by the agent). The last output lines and `blocked.md` go into the report.
- Silence: after `idle_warn_min` without output an `idle` event is emitted (a warning only; Claude Code can be silent while waiting on a rate limit). The hard stop is `agent_timeout_min`.
- Whatever the agent committed itself is normalised: the orchestrator diffs against the recorded base commit and creates the single commit itself.

## Verifier: three gates

1. **Fast verifier (inner loop, runs in the agent).** One command, named in `task.md`, that the agent runs itself after each change. It runs from the copy under `.agent/tools/` inside the workspace, because a confined agent cannot execute files outside it. `{tools}` resolves to the workspace copy in the inner loop and to the trusted original in the outer gate. The agent can tamper with its copy, which is harmless because the outer gate is authoritative. The verifier's own child processes (e.g. package caches) may still need paths outside the workspace; test that. It must be fast (formatting, lint, unit tests, quick static checks). Output shown to the agent is compact: exit code, few lines on success, on failure the lines matching `error_patterns` plus the last `tail_lines`, so the agent's context is not flooded. The full output goes to `verify-<n>.log`.
2. **Outer gate (orchestrator).** After the agent ends, the same verifier runs again from the trusted copy (because the agent can edit files in the repo, including a verifier script inside it), then the guards run. Only if both pass does anything get pushed.
3. **CI gate.** Integration tests run in the repository's own CI on the pushed commit. The gate is defined in "CI layer and CI gate" below. **Merging starts only after CI is green on the exact commit that was pushed.**

**Merge is a two-key operation and off by default:** the recipe must set `merge = "when_green"` *and* the run must be started with `--merge`. The merge is done by the orchestrator through the host API and respects branch protection and approval rules. If the host refuses, the repo ends as `merge_blocked`; fleetify never forces a merge. Without both keys the run ends at `pr_open` (no CI wait) or `ci_passed` (CI green) and a human merges.

**Delivery ladder (opt-in, one step at a time).** Private repositories get the most conservative setting by default; each further step has to be asked for in the recipe, and the steps are validated against each other:

1. `delivery.open_mr = false`: push the branch and stop. Result `pushed`. Nothing is opened, nothing is merged, nothing is deleted.
2. `delivery.open_mr = true` (default): push and open the pull or merge request. Result `pr_open`. A human merges.
3. `ci.merge = "when_green"` (needs `ci.wait`, so it needs M11) together with `--merge` at run time: merge after CI is green on the pushed commit, with the expected-head guard.
4. `delivery.delete_branch = true` (needs step 3): delete the source branch, but only after the merge was confirmed. Not deleted on any other result.

Each step is refused at validation if the step before it is off: `when_green` needs `open_mr`, and `delete_branch` needs `when_green`. A push-only run that finds its branch already on the remote ends as `already_done`, so it never pushes or opens anything over an earlier run.

Stage 1 only reports CI failures. Feeding a CI failure back to the agent as a new attempt (CI-fix loop) is stage 2.

## CI layer and CI gate

**Purpose.** The CI gate is the last check before any merge: the repository's own CI must pass on the exact commit fleetify pushed. It is separate from the verifier. The verifier is fast, local and runs inside the agent loop. CI runs the integration tests and everything slow, in the company's own environment.

**CI is separate from the host.** The CI system can differ from the Git host, so CI has its own trait, one module per CI system, reusing the host's API client and credential:

```rust
pub trait CiProvider {
    fn name(&self) -> &str;
    fn pipelines(&self, repo: &Repo, sha: &str, cr: Option<&ChangeRequest>) -> Result<Vec<Pipeline>>;   // everything CI knows about this commit
    fn state(&self, pipelines: &[Pipeline]) -> CiState;                     // None | Pending | Success | Failed | Blocked
    fn failure_details(&self, repo: &Repo, pipelines: &[Pipeline], max_lines: usize) -> Result<Vec<FailedJob>>;   // name, stage, url, log tail
}
```

**Stage 1 supports GitLab CI only** (`ci/gitlab_ci.rs`). `provider = "auto"` selects it for GitLab repos. For a GitHub repo, `wait = true` fails validation before anything runs ("no CI provider for this host yet, set wait = false or contribute one"); GitHub flows end at `pr_open`. Other CI systems (GitHub Actions, Jenkins) are one module each and can come later or from the community.

### The gate, independent of the provider

Settings (`[ci]` in the recipe): `provider` (default `"auto"`), `wait` (default `false`; without it there is no CI check and the run ends at `pr_open`), `appear_timeout_min` (default 5), `timeout_min` (default 30, counted from the push), `poll_interval_s` (default 30, with jitter and back-off on rate limits), `failure_log_lines` (default 40), `merge` (`"off"` | `"when_green"`).

1. After the push and the PR/MR creation, poll until the state is final or a timeout hits. Polling is a separate task: the agent slot is free once the push is done, and waiting time is not agent time.
2. **No pipeline within `appear_timeout_min`: `ci_missing`.** No CI never counts as green. Typical causes: no pipeline configuration, rules that exclude the branch or merge requests, skip markers, a blocked pipeline creation. fleetify never adds skip-CI markers to commit messages, and recipe validation rejects a commit message or PR title containing one.
3. **Aggregation:** consider every pipeline of the exact pushed commit (branch pipelines and merge request pipelines). The gate is green only if at least one exists, none is still pending or running, and all of them succeeded. Any failed or canceled pipeline is `ci_failed`. A pipeline waiting for a manual job is `ci_blocked` (needs a human; fleetify never triggers manual jobs). Skipped pipelines do not count as success; if only skipped ones exist the result is `ci_missing`.
4. Overall timeout: `ci_timeout` (the pipeline may still be running; the report carries the link).
5. **The result belongs to the pushed commit.** Before a merge the head of the MR must still be that commit (see below).
6. No re-triggers and no retries of jobs in stage 1, not even for flaky jobs.
7. **Failure details:** on `ci_failed` the failed jobs (name, stage, URL) are recorded as events and in the report, and optionally the last `failure_log_lines` lines of each failed job's log. That text is untrusted: redacted, length-limited, displayed only, never executed and not handed to the agent in stage 1 (the CI-fix loop in stage 2 will).
8. Green CI without a merge ends as `ci_passed`.

### GitLab CI provider (first implementation)

- Uses the REST API: list the project's pipelines for the commit SHA, read their status and jobs, and read job logs for failure details. Endpoints and parameters must be checked against the API documentation of the deployed GitLab version; they have not been verified here.
- Also include the merge request's own head pipeline, in union with the pipelines found by SHA.
- Status mapping (to be verified against the docs): `created`, `waiting_for_resource`, `preparing`, `pending`, `scheduled`, `running` map to pending; `success` to success; `failed`, `canceled` to failed; `skipped` to skipped; `manual` to blocked. An unknown status is treated as pending, logged as a warning, and ends as `ci_timeout` if it never resolves.
- The credential needs read access to pipelines and jobs; merging needs more. Check the exact token scopes in the docs and write them into the README's credentials section.
- **Not covered in the first version:** results that arrive only as external commit statuses (for example from Jenkins), merge trains and merged-results pipelines, per-job "required" lists, and special handling of `allow_failure` jobs (the pipeline's own aggregate status is used as is). Downstream pipelines count only as far as the pipeline status already includes them.

### Merge after CI

Merging stays a two-key operation (`merge = "when_green"` and `--merge`). In addition: immediately before merging, the orchestrator re-reads the MR head and requires it to equal the pushed commit and the gate to be green for that commit. The merge call passes that commit as the expected head (the GitHub and GitLab merge APIs support such a guard; verify in the docs). A different head means `merge_blocked` with the reason in the event log and report. The host's branch protection, approvals and required-pipeline rules apply as usual, and any refusal is `merge_blocked`; fleetify never forces a merge.

### Tests for the CI layer

Against a fake GitLab API: success; failed; canceled; no pipeline ever; only skipped; manual-blocked; slow pipeline until timeout; pipeline appearing late; branch pipeline and MR pipeline with one failing; rate limit (429) while polling; head commit changed before merge; unknown status. Plus one real run against a private gitlab.com project with a trivial pipeline, and later one against the self-hosted GitLab at work.

## Availability: outages of hosts and CI

Hosts and CI servers can be unreachable for minutes or hours: the company GitLab is only reachable inside the network, GitHub and gitlab.com have outages, and a scheduled run may start while a server is down. The rule is that an outage never produces a false result and never produces a merge.

1. **Transient errors are retried, not fatal.** Connection errors, timeouts, HTTP 5xx and 429 (with `Retry-After` when present) are retried with exponential back-off and jitter, inside the same budget as the CI gate (`timeout_min`). The current code ends the repo as `error` on the first failed request in `ci::poll`; that is wrong for these errors. Other 4xx answers stay immediate errors.
2. **No verdict from partial data.** A poll that could not read every item of the commit gives no verdict for that poll. Only a complete, successful read can end the gate as `ci_passed`. A failed read never counts as green, pending or missing.
3. **Outage beyond the budget: `host_unreachable`.** A new result, not successful, with a `failure` block that names the host, the operation, the last error, the last time a read succeeded, the pushed SHA and the change request URL. It is distinct from `error` (a bug or a bad answer) and from `ci_timeout` (the pipeline is still running). Exit code 1 as for other unsuccessful results.
4. **Resume is the normal path.** A re-run with the same run id reads `state.json`, sees the pushed SHA and the change request, and continues at the CI gate for that SHA (rule 3 of "Run flow per repo"). It never pushes again and never opens a second change request. The CI verdict of an earlier poll is stored in `state.json` with its time, so a re-run after a long outage shows what was known before.
5. **Merge is never retried blindly.** Before a merge the orchestrator reads the change request. If it is already `merged`, the result is `merged`, not `merge_blocked` (a merge call may have succeeded on the server while its response was lost). If the host is unreachable when the merge is sent, the repo ends as `host_unreachable` and the merge is repeated only by a re-run, which starts with the same check. The expected-head guard stays on every attempt.
6. **Other repos go on.** An outage of one host ends only the repos that use it. With `--parallel`, repos on other hosts keep running. The run does not abort, and the report lists the affected repos with `host_unreachable`.
7. **Git operations get the same treatment.** `ls-remote`, `fetch` and `clone` are retried like API calls. A `push` that timed out is checked before it is repeated: if the remote branch already points at the pushed SHA, the push is done; if it points elsewhere, the repo ends as `error` (a branch with foreign commits is never overwritten).
8. **Automated callers can act on it.** The report carries `host_unreachable` with the host and the time of the last successful read, so a scheduler can rerun later. The exit code does not change: 1 means "not successful", and the report says why.
9. **Visible in the event stream.** Every retry is an event (`host_retry` with host, operation, attempt, delay, error class), so a live view or a log can show a server being down without treating it as a failure.

**Tests** (against the fake host and fake CI server): a read that fails twice and then succeeds gives the normal verdict; a 429 with `Retry-After` is honoured; an outage longer than `timeout_min` gives `host_unreachable` with the failure block; a re-run after recovery continues at the gate without a second push or change request; a merge response lost after the server merged gives `merged` on the re-run; a push whose remote branch points elsewhere is refused, not overwritten; a repo on an unreachable host does not stop a repo on a reachable host.

**Why this is stage 1, not stage 2:** the company GitLab is only reachable inside the company network, and scheduled runs will hit outages. Without rules 1–5 a single network blip gives a wrong result and a retry gives a duplicate or a missed merge.

### Rate limits (extension of M11a)

Rate limits come from two sides, and each needs its own handling. Neither is implemented yet.

**Hosts and CI (fleetify's own requests).**

1. **Use the server's answer.** `Retry-After` (seconds or HTTP date) sets the wait when present. The rate-limit headers of the host set it otherwise: when the remaining quota is used up, wait until the reset time instead of guessing with back-off. Header names are taken from the current API docs of each host and noted in the host module (GitHub: `x-ratelimit-remaining`, `x-ratelimit-reset`; GitLab: `RateLimit-*`; to be verified).
2. **Secondary limits count too.** GitHub reports some limits as HTTP 403 with its own message. Those are rate limits, not access errors, and must not end as `Forbidden`.
3. **Shared per host.** Parallel repos on one host share one limiter. When one repo has to wait, the others on that host wait too instead of hitting the same limit again.
4. **Configured ceiling.** `config.toml`: `[rate_limit] max_wait_min = 15` is an absolute maximum for one wait. A wait longer than that is not taken; the repo ends as `rate_limited`. Without the key a default ceiling applies, so the run never waits without bound. An optional proactive limit (`max_requests_per_min` per host, off by default) can be added later.
5. **Own result.** `rate_limited` is a new result, not successful. It is distinct from `host_unreachable` (the server did not answer) because the server did answer and told us when to come back. Its failure block names the host and the reset time. A re-run after the reset continues, as for outages.
6. **Visible.** Each wait is a `rate_limited` event with host, wait time and reason, so a live view or a log shows it without treating it as a failure.

**Agents (the coding agent's own rate limits).** The agent's connection to its model provider is not fleetify's connection. Claude Code waits and blocks on its own; opencode and Copilot CLI behave differently and are not known yet. Today the only protection is the idle warning and `agent_timeout`, which ends the repo as a generic timeout and hides the cause. Planned:

7. **Classify by adapter.** `Agent::classify` recognises the adapter's rate-limit message in the output and returns a rate-limit outcome. The repo ends as `rate_limited` with the message in the failure block, not as `agent_timeout`.
8. **Ceiling for agents too.** The same `max_wait_min` applies if the agent is known to wait for its limit. An agent that waits longer than the ceiling is stopped and the repo ends as `rate_limited`; a re-run continues.
9. **What can be tested.** Classification and the result mapping are tested with fake agents that print a rate-limit message and exit (no real limit needed). The real behaviour of each CLI is observed only in the level-5 runs (M14 for Claude Code) and documented there.

**Tests** (fake host and fake agent): a `Retry-After` value is honoured; a reset header is used instead of back-off; a wait above `max_wait_min` ends as `rate_limited` with the reset time; a 403 with the secondary-limit message is a rate limit; two repos on one host wait on the same limiter; a fake agent printing a rate-limit message ends as `rate_limited`, not `agent_timeout`; a re-run after the reset continues.

**Acceptance** is added to M11a: every test above passes on both OS, and the M14 level-5 run documents Claude Code's observed behaviour when it hits its own limit.

## Guards (diff guards)

Guards decide whether the agent's diff may leave the machine. They are layered so that the cheap, reliable layer always runs and the expensive, fuzzy layer is optional.

**Layer 1: deterministic rules (always on, no LLM).** Evaluated on the diff against the base commit (computed with line-ending differences ignored). Fields: `allow_paths` (optional whitelist; anything else fails), `deny_paths`, `deny_added_patterns`, `max_files_changed`, `max_lines_changed`, `forbid_test_deletion` (deleting or renaming-away a test file fails; file level, by path pattern; weakened test cases are the judge's job), `forbid_binary_files`. `.agent/**` is always denied regardless of the recipe. A violation ends the repo as `guard_violation` with the rule and the offending lines in the report. No push, workspace kept.

**Layer 2: LLM judge (optional, off by default).** Answers questions rules cannot: is this diff what the task asked for, or did the agent refactor unrelated code, weaken a check or "fix" the symptom?

- **What it is:** a separate, fresh, one-shot LLM call. Not the executing agent's session, no tools, no repo access beyond what is in its input. Input: the assembled task text, the diff, and the verifier summary. Output: a JSON verdict `{"verdict": "pass" | "fail" | "unsure", "reasons": [...]}`, validated against a schema. A parse error counts as `unsure`.
- **Which model, which provider:** your choice, configured independently of the executing agent: `[judge] agent = "opencode", profile = "judge", max_diff_lines = 1500`. The call goes through one of the three supported agent CLIs via `Agent::judge_command()` (one-shot, read-only, no workspace), so the model and provider come from that CLI's own configuration (fleetify configures nothing). `profile` is an opaque selector that the adapter passes on if the CLI supports named profiles or agents (check each CLI's docs); otherwise the CLI's default model is used. Recommendation: use a *different* model, ideally a different provider, than the executor to reduce correlated blind spots. Constraint: the diff is company code, so the judge may only use providers approved for that code (at work likely Copilot only).
- **One-directional:** the judge can only veto. `fail` ends the repo as `guard_violation`, `unsure` ends it as `needs_review` (nothing is pushed, workspace kept). A `pass` never overrides layer 1 or the verifier.
- **Untrusted input:** the diff is written by the agent and may contain instructions aimed at the judge. It is passed as fenced data, the judge has no tools, and its output is schema-checked.
- **Big diffs:** over `max_diff_lines` the verdict is `unsure`, never a silent truncation.
- **Prompt:** a static, versioned file embedded in the binary, with a checklist (unrelated changes, disabled or weakened tests/checks, suppression comments, secrets, scope creep).
- **Evaluation:** a fixture set of good and bad diffs. The judge is only enabled for a recipe after it catches the bad ones on that set. Cost and latency are recorded per call.

## Commands, shells and operating systems

- Commands are argument lists (`argv`, or `posix`/`windows` variants). There is no implicit shell. Placeholders (`{repo}`, `{tools}`) are substituted per element, never into a concatenated string.
- If a shell is needed it is explicit in the argument list (`pwsh`, `bash`, `sh`, `cmd`). On Windows PowerShell is the primary case: prefer `-File` with arguments over `-Command` strings to avoid quoting problems. Execution policy follows the machine's setup; fleetify does not bypass it.
- Child output is read as UTF-8 with replacement for invalid bytes. Line endings of repository files are preserved: fleetify must not convert them, and the guards ignore pure line-ending noise.
- Timeouts terminate the whole process tree: Job Objects on Windows, process groups on POSIX (`command-group` or equivalent; verify it actually kills grandchildren of `pwsh`). This needs tests on both.
- Paths use `std::path`, never hard-coded separators.
- Every `git` call goes through the same runner (so it is logged, timed out and redacted like everything else).
- The binary's own test suite runs on Windows and Linux (CI matrix); release builds are produced for `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu` (and `aarch64-apple-darwin` if a Mac is around).

## Run flow per repo

1. Clone or fetch the cache clone, resolve the default branch, record its SHA as the base commit.
2. `applies_to` false (evaluated on the cache clone): result `skipped`.
3. Idempotency check against the host:
   - open PR/MR for the branch exists → `already_done`, except: if `ci.wait` is set and this run's `state.json` has no CI result for that PR's head, resume at step 10 (CI gate) for that head.
   - remote branch exists but no open PR/MR (e.g. a crash between push and PR creation) → resume at step 9b: open the PR/MR on the existing branch, no new commit.
   - this run's `state.json` shows a finished or interrupted workspace for this repo → the workspace is deleted and the repo starts over at step 4 (the agent's attempt is not reused in stage 1).
4. Create the workspace (`git clone --local` from the cache, `origin` = real remote, invalid `pushurl`, `.agent/` excluded), create the branch from the base commit.
5. `prepare`, if given.
6. If `instructions` exist: copy `tools` and attachments into `.agent/`, write `.agent/task.md`, start the agent (stdin closed), stream output, watch idle time and timeout, watch for prompts and `blocked.md`. Collect `result.md` / `blocked.md`.
7. No changes against the base commit: result `no_changes`.
8. Outer gate: verifier (trusted copy) and guards (and judge, if enabled).
9. (a) Single commit made by the orchestrator, push via `GIT_ASKPASS`. (b) Open PR/MR against the default branch. The PR body names the recipe, includes the verifier summary and the agent's `result.md` (clearly marked as agent-written, length-limited). Without `ci.wait`: result `pr_open`.
10. CI gate for the pushed commit (see "CI layer and CI gate"). Green without merge: `ci_passed`.
11. Optional merge (two keys): `merged` or `merge_blocked`.
12. Append the final result to `results.jsonl`; when the last repo finishes, write `report.json`.

Phases: `queued, cloning, preparing, agent, verifying, pushing, ci, merging`. Final results: `pr_open, ci_passed, merged, skipped, already_done, no_changes, verify_failed, guard_violation, needs_review, needs_input, blocked, agent_timeout, ci_failed, ci_missing, ci_blocked, ci_timeout, merge_blocked, interrupted, error`.

**Successful results** (used by `--continue-if-green` and the exit code): `pr_open` when `ci.wait` is off, `ci_passed`, `merged`, `skipped`, `already_done`, `no_changes`. Everything else counts as not successful.

## Rollout in waves (ramp-up)

Real runs over more than one repo go in waves. Default: `--waves 1,all`.

- Wave 1 is one repo (or `--canary-repos a,b` to choose representative ones). It runs through all configured gates, including CI if `ci.wait` is set.
- After a wave the tool prints the results and asks `Continue with N remaining repos? [y/N]` when interactive.
- Non-interactive: it stops after the wave and exits with code 2; `--continue-if-green` continues automatically only if every repo of the wave reached a successful result. Otherwise it stops with a non-zero exit code.
- `--waves 1,5,all` gives more steps, `--waves all` skips the ramp-up.
- Because runs are idempotent, continuing simply skips `already_done` repos.

**Exit codes:** 0 all repos successful; 1 at least one repo not successful; 2 stopped after a wave waiting for a human; 3 refused before starting (validation, permissions, credentials, unchecked agent); 4 the tool itself crashed (see "Crashes of the tool").

## Events, renderers and the report

**One event stream.** The orchestrator never prints. It emits typed events (`serde`-serialisable enum) into a broadcast channel: `run_started` (recipe, run id, effective permissions, config hash), `repo_phase` (repo, phase), `command_started` / `command_finished` (argv redacted, exit code, duration), `output_line` (repo, stream, line), `verifier_result`, `guard_violation`, `judge_verdict`, `agent_idle`, `ci_poll`, `ci_result`, `repo_finished` (repo, result, details), `wave_finished`, `run_finished`. Consumers subscribe to the channel; the core does not know who is listening.

**Consumers in stage 1:**

- **Event log writer:** `events.jsonl` per repo, append-only, one JSON object per event. Always on.
- **State writer:** `state.json` per repo (current phase, start time, last output time, last N output lines, result, pushed SHA, PR/MR URL, CI result), written atomically (temp file and rename) so another terminal can read it at any time. Always on.
- **Log file writers:** `agent.log`, `verify-<n>.log`, `prepare.log`, `task.md` (as rendered), `diff.patch`, `result.md` / `blocked.md` if present. Always on.
- **Report writer:** builds `report.json` from the final events; see below. Always on.
- **Plain renderer:** prefixed lines like `docker compose logs` (`repo-name | line`), phase changes, a periodic summary line (e.g. every 60 s: `5 running, 2 done, 1 failed`), and a final table. Default whenever stdout is not a TTY or `--plain` is given. Everything goes to stdout; the tool's own diagnostics go to stderr.
- **TUI renderer (optional, `--ui`):** see "Live view".

Nothing a renderer shows comes from anywhere but the event stream, so the live view can never contain information that is missing from the logs and the report.

**Report (`report.json`).** The primary output for automated use, written at the end of the run and rewritten on every re-run of the same run id. Structure:

```json
{
  "schema": 1, "run_id": "...", "recipe": "...", "started": "...", "finished": "...", "exit_code": 1,
  "summary": {"total": 12, "successful": 10, "by_result": {"pr_open": 9, "skipped": 1, "verify_failed": 2}},
  "repos": [
    {
      "repo": "gitlab.com/group/sub/project-c", "result": "verify_failed", "successful": false,
      "phases": {"cloning": 3.1, "agent": 412.0, "verifying": 18.4},
      "change_request": null, "pushed_sha": null,
      "failure": {
        "class": "verify_failed",
        "message": "verifier exited with 1 (trusted copy)",
        "rule": null,
        "output_tail": ["...last 40 lines of verify-2.log..."],
        "agent_result_md": "...length-limited...", "agent_blocked_md": null,
        "artifacts": {"events": "…/events.jsonl", "agent_log": "…/agent.log", "verify_log": "…/verify-2.log", "diff": "…/diff.patch", "workspace": "…"}
      }
    }
  ]
}
```

`failure` is present for every non-successful repo and carries the one thing a caller needs to decide what to do next: for `guard_violation` the rule and offending lines, for `ci_failed` the failed jobs with URLs and log tails, for `needs_input` / `blocked` the prompt or `blocked.md`, for `agent_timeout` the last output lines, for `error` the error chain. All text in it is redacted and length-limited. `fleetify run --format json` prints the report to stdout instead of the table; `--report PATH` writes a copy elsewhere. `fleetify report [--run NAME] [--format table|json] [--failed]` renders the same from the run folder later.

**Crashes of the tool.** `tracing` with a JSON subscriber writes `fleetify.log` into the run folder (level `info` by default, `--debug` for `debug`, `RUST_LOG` honoured). A panic hook writes the panic message and backtrace to `fleetify.log` and stderr, terminates all child process trees, marks running repos `error` with the panic message as failure, still writes `report.json`, and exits with code 4. `--debug` additionally keeps every subprocess's full output in the events.

**`fleetify status [--run NAME] [--watch]`** shows a table from the state files (repo, phase or result, elapsed, idle time, last line, PR/MR link), usable from a second terminal while a run is active. Default run: the most recently modified. **`fleetify logs <repo> [--run NAME] [-f] [-n 50]`** tails one repo's agent log.

**Ctrl-C:** terminates all child process trees, marks running repos `interrupted`, keeps workspaces, writes the report, prints the summary. A re-run continues according to the resume rules.

## Live view (optional, demo)

`--ui` (only when stdout is a TTY; otherwise ignored with a note) starts a ratatui renderer on the event stream:

- One screen, no navigation: a table with one row per repo (`repo`, `phase`/result, `elapsed`, `idle`, last output line); active repos first, finished repos sorted below; a footer with the running summary (`5 running, 2 done, 1 failed`) and the wave prompt when a wave ends.
- A repo whose last event is a problem (idle too long, `needs_input`, `blocked`, `verify_failed`, `guard_violation`, CI failed) is highlighted and expands to show the last `--tail N` lines (default 6) of the failing output.
- `q` or Ctrl-C behaves like Ctrl-C in plain mode. Respect `NO_COLOR`. On any terminal error it falls back to the plain renderer without losing events.
- Deliberately not in scope: scrolling through logs, switching views, editing anything. `fleetify logs` and the report cover that.

## CLI (stage 1)

```
fleetify run <recipe-dir> repos.txt [--dry-run] [--only a,b] [--parallel 3] [--run NAME]
            [--agent NAME] [--waves 1,all] [--canary-repos a,b] [--continue-if-green]
            [--merge] [--allow-unchecked-agent] [--plain | --ui] [--tail N]
            [--format table|json] [--report PATH] [--debug]
fleetify validate <recipe-dir>     # schema check, resolve files, print rendered commands per OS
fleetify schema                    # JSON Schema of recipe.toml for this binary
fleetify report [--run NAME] [--format table|json] [--failed]
fleetify status [--run NAME] [--watch]
fleetify logs <repo> [--run NAME] [-f] [-n 50]
fleetify check-agent NAME          # canary test: is the agent confined to its workspace?
fleetify clean [--run NAME]        # delete workspaces of a run (clones stay)
```

`--dry-run` clones or fetches the cache, evaluates `applies_to`, shows per repo what would happen (including the idempotency verdict), which credential rule matches, the effective permissions per agent, and checks read access at the host. It creates no workspace, starts no agent and pushes nothing. It produces a report like a real run, with result `dry_run` per repo.

## Security (minimum)

- Push, PR/MR, CI polling and merge are done only by the orchestrator. The agent gets no host tokens. In the agent's workspace `remote.origin.pushurl` is set to an invalid value; the orchestrator pushes via the explicit URL. That is a hurdle, not a sandbox.
- The agent process gets an environment allowlist, not the user's full environment.
- Agents are started with explicit permissions: workspace only, allowlisted commands (see "Permissions and workspace restriction"). An agent and version without a passing canary check triggers a warning; with `require_checked_agent = true` it is refused unless `--allow-unchecked-agent` is set.
- Agent CLIs often run with broad permissions (auto-approve). Acceptable for private repos, for company repos only inside a container without credentials (stage 2).
- Inputs from repos (branch names, file names, scanner messages) never flow into shell strings.
- Every process has a timeout; on expiry the whole process tree is terminated.
- Everything the agent wrote (including `result.md`) is untrusted data: displayed and length-limited in the report, never executed or interpreted as instructions.

## Testing and automation

| Level | What | Where | When |
|---|---|---|---|
| 1. Unit | `repos.txt` parsing, recipe parsing and validation, credential matching, command building incl. Windows quoting, guards, redaction, event serialisation, report building | CI matrix Windows + Linux | every commit |
| 2. Pipeline E2E with fakes | local bare repos as remotes, a fake host and CI HTTP server (merge requests, pipelines, jobs, logs), fake agent scripts (apply a patch, hang, ask a question, crash, write `blocked.md`, make forbidden changes, commit by itself, read or write outside the workspace to test the canary check); assertions on `report.json` and `events.jsonl` | CI matrix Windows + Linux | every commit |
| 3. E2E with a local model | opencode with a llama.cpp server; asserts pipeline invariants only, not model quality | nightly job or a workstation (needs a model download and CPU/GPU time) | nightly, before release |
| 4. Real hosts | throwaway repos on GitHub and a private project on gitlab.com (with a trivial pipeline); a test project on the self-hosted GitLab (only reachable inside the company network) | scheduled job with secrets; GitLab part inside the company by hand | weekly or manual |
| 5. Real agents | Claude Code (private), Copilot CLI (work) on test repos | manual script (`scripts/e2e-*`) | before a release (costs tokens, rate limits) |

Levels 1 and 2 are cheap to automate fully and give most of the confidence, so they come first. Level 3 is added once an agent adapter exists. Levels 4 and 5 are repeatable scripts run by hand at first. How much further to automate is decided after stage 1.

Invariants every E2E level asserts: no push when a gate failed; failed, missing or blocked CI never leads to a merge; no change outside the workspace; secrets never in logs, events or the report; timeouts leave no running processes; a re-run creates no duplicate branches or PRs; every non-successful repo has a `failure` block in the report with a non-empty message; `verify_failed`, `agent_timeout`, `needs_input` and `blocked` are valid outcomes while a PR despite a failed gate is a bug.

Company repos probably must only be processed with the agent approved there (Copilot). That needs clearing with compliance and is not a detail of the tool.

## Stage 1: implementation

Each milestone is one branch and ends with its acceptance criteria green on the Windows + Linux CI matrix. "Depends on" lists hard prerequisites; milestones without a dependency between them may be done in any order or in parallel. `[ASK]` marks a milestone where Claude Code asks before running anything against real repos, real agents or a merge.

**M9 — Agent runner with fake agents.** Depends on M7. `Agent` trait, `Permissions` model, baseline + config grants + `needs` check, effective-permissions printing and `permissions_effective` event, `task.md` assembly (rules block, instructions, attachments, verifier command), `.agent/tools` copy, agent start with allowlisted environment and stdin closed, output streaming into the event stream, `wants_input()` polling, idle event, agent timeout, `blocked.md`/`result.md` handling, normalising agent-made commits, results `needs_input`, `blocked`, `agent_timeout` with `failure` blocks; the `fake` adapter driven by scripts. *Acceptance:* fake agents (apply a patch, hang, ask a question, crash, write `blocked.md`, forbidden change, commit by itself, write outside the workspace) each produce the specified result and failure block; a hanging fake agent is terminated with its children on both OS; a recipe whose `needs` exceed the effective permissions is refused before starting (exit 3) with the config entry named; the agent process environment contains only the allowlist.

**M10 — opencode adapter and `check-agent`.** Depends on M9. Check opencode's current docs for the non-interactive invocation and permission options before writing the adapter; map `Permissions` to them and report `unsupported` entries; `env()` allowlist for opencode's config; `judge_command()`; `check-agent` with canary file, per-version cache, `require_checked_agent`, `--allow-unchecked-agent`. *Acceptance:* the adapter builds its command with the baseline grant and `--dry-run` shows the effective permissions and gaps; `fleetify check-agent opencode` passes (or the gap is documented and `unrestricted = true` is required for the smoke test); `[ASK]` level-3 smoke test with opencode and llama.cpp on a throwaway GitHub repo runs through and the invariants hold whether or not the local model solves the task.

**M11 — CI layer, CI gate and merge.** Depends on M6, M7 (can run in parallel with M9–M10). `CiProvider`, `ci/gitlab_ci.rs`, the gate as a separate `tokio` task, failure details into events and report, results `ci_passed`, `ci_failed`, `ci_missing`, `ci_blocked`, `ci_timeout`, resume at the CI gate, two-key merge with the expected-head guard, `merged` and `merge_blocked`, validation error for `wait = true` on GitHub. *Acceptance:* every scenario under "Tests for the CI layer" produces the specified result against the fake GitLab API and the matching `failure` block; failed, missing or blocked CI never leads to a merge; a merge happens only with both keys and a matching head; `[ASK]` one real run against the private gitlab.com project with a trivial pipeline ends as `ci_passed`, and one with `--merge` on a throwaway branch ends as `merged`.

**M11a — Outage handling.** Depends on M11. Retry with back-off for host and CI reads (rule 1), no verdict from partial reads (rule 2), `host_unreachable` with its failure block (rule 3), `host_retry` events, merge check before merging and retry-safe push check (rules 5 and 7), and the resume behaviour of rule 4. *Acceptance:* every test listed under "Availability: outages of hosts and CI" and under "Rate limits (extension of M11a)" passes against the fake host and fake CI server on both OS; a `host_unreachable` result carries a non-empty `failure` block; a `rate_limited` result carries the reset time; no test sends a second push or a second change request. Must be done before the first real run against the company GitLab (M15).

**M12 — Python authoring package (optional).** Depends on M3. Separate `uv` project `fleetify-recipes`: dataclasses mirroring the TOML schema, `write()`, validation against `fleetify schema` output, one example that reproduces the `enable-gitlab-ci` recipe byte-for-byte. *Acceptance:* the generated `recipe.toml` passes `fleetify validate` and equals the hand-written one; a schema-version mismatch is an error.

**M13 — Live view (optional, demo).** Depends on M9. ratatui renderer on the event stream as specified under "Live view", `--ui`, `--tail`, `NO_COLOR`, fallback to plain. *Acceptance:* manual check on Windows Terminal and a Linux terminal with the fake agents; piping to a file ignores `--ui` and produces plain output; the level-2 tests are unchanged.

**M14 — Claude Code adapter (private).** Depends on M10. Same procedure as M10: check current docs, map permissions, allowlist, `judge_command()`, `check-agent`. `[ASK]` Level-5 test: fix a small, deliberately introduced lint violation on a throwaway GitHub repo and open a PR that passes the gates. Observed rate-limit behaviour is documented. *Acceptance:* the test above and a passing or documented `check-agent` result.

**M15 — Copilot CLI adapter and self-hosted GitLab (at work).** Depends on M10, M11 (both done for GitHub; GitLab CI is done as well). Split into two parts, because Copilot CLI is available privately now (Copilot Free, CLI 1.0.101 in `~/.local/bin`).

- **M15a — Copilot adapter (private, now).** `agents/copilot.rs`, same procedure as M14 (docs checked on 2026-10-21, date and URL in the module doc comment). Command: `-p <one-liner> -s --no-ask-user --output-format=json` (`--output-format` to be checked in the run). Permissions map to specific grants only: `Permissions` directories become `--allow-tool='write(<dir>/*)'` per granted directory, never a bare `write`; commands become `shell(<command>)` entries; web becomes `url(...)`. `--allow-all`, `--yolo` and `COPILOT_ALLOW_ALL` are never used unless `unrestricted = true`. `env()` allowlist without any host token; the agent's own login comes from its own config. `unsupported()` reports grants the CLI cannot express. `judge_command()` one-shot without tools. Tests: unit tests for the command building (grants per directory, no bare write, no allow-all), fake-agent tests for the outcome mapping, `check-agent copilot` with the canary. *Acceptance:* `check-agent copilot` passes with the baseline grants, a real level-5 run on a throwaway private GitHub repo opens a PR that passes the gates, and the observed Copilot behaviour (exit codes of `-p`, what happens without a grant, rate-limit messages) is written into the module doc comment and DECISIONS.md.
- **M15b — Self-hosted GitLab (at work).** Same adapter, on the company's self-hosted GitLab on Windows with the internal CA (MR opened, CI gate evaluated), with Copilot's own login (never the host token). *Acceptance:* the self-hosted GitLab run opens one MR and reports the CI result; the README credentials section lists the required token scopes.

**M16 — Optional: LLM judge.** Depends on M9 (and an adapter from M10/M14/M15). `[judge]` in the recipe, embedded prompt, schema validation, size limit, fixture set, cost and latency recording. *Acceptance:* catches the bad fixtures (suppression comment, deleted test case, unrelated refactor, weakened check) and returns `unsure` on oversized diffs and on malformed output; `fail` → `guard_violation`, `unsure` → `needs_review`, nothing pushed.

**M17 — Real trial.** Depends on M8, M10 or M14, M11. `[ASK]` A recipe with an agent on three to five private repos with `ci.wait` on the GitLab ones, run once in automated mode (plain, `--format json`) and once with `--ui` for the demo. Evaluate from `report.json` and the event logs: how many repos went through without intervention, time per repo, where help was needed, whether the failure blocks were enough to act on without opening the run folder. Write the numbers into `TRIAL.md`.

**Suggested order for one person (remaining):** M11a (rate limits: hosts done, agents open) → M15a (Copilot adapter, private) → M17, with M12 when a second recipe is written, M15b at work, and M16 only if the trial shows the deterministic guards are not enough.

## Decision after stage 1

If most repos go through without intervention, extending is worth it. If the result is thin, improve prompt and verifier first. If it stays thin, Armada or the SonarQube Remediation Agent may reach the goal faster than building further. The numbers from `report.json` and the event logs are the basis.

## Stage 2: only when needed

- SQLite instead of files, per-repo attempts, `retry` / `skip` with reasons, `takeover`, reusing an interrupted agent attempt.
- Rollback (close PR/MR and delete branch, revert PR/MR after a merge) driven by a registry of created artifacts.
- CI-fix loop (feed CI failures back to the agent).
- More CI providers (GitHub Actions, Jenkins via commit statuses), job-level policies, retries of flaky jobs.
- Campaign time limits, circuit breaker, rate-limit governor, event-stream agent adapters.
- Agents inside containers without credentials (required for company repos).
- Tokens from the OS credential store; optional CLI variants of the host layer.
- Judge evaluation over time, failure classes, webhook on `needs_human`.
- An embedded scripting language (Rhai) for `applies_to`, only if the condition list proves insufficient.

## Stage 3: only if it keeps paying off

`verify` as an MCP tool, plan SDK with JSON plans, broadcast shell, web UI, statistics per recipe version. See `PLAN-ausbau.md`.

## Non-goals

- Other hosts than GitHub and GitLab, other agents than Claude Code, opencode and Copilot CLI. Anyone who needs them implements the `Host` or `Agent` trait.
- CI systems other than GitLab CI in stage 1. A `CiProvider` is one module and can come later or from the community.
- Existing local checkouts.
- Logic in the recipe format. Logic lives in `tools/` scripts.
- Running Python at runtime. The Python package only generates files.
- Interactive multi-terminal views (tmux style), log browsing in the TUI.
- Configuring an agent's provider, model or login. (The permissions in the invocation are fleetify's job and in scope.)

## Open questions before starting

1. **GitLab/GitHub at work.** Settled: self-hosted, current, regularly updated, REST works, internal CA installed. Open: which tokens will exist (fine-grained per repo, group, user) and who manages them? Which role allows pushing branches and creating MRs? Are `chore/*` branches protected? Is there a corporate proxy (the HTTP client must honour `HTTPS_PROXY`)? Is the GitHub at work github.com or GitHub Enterprise Server?
2. **Agents.** Settled: Claude Code and opencode (with llama.cpp) privately, Copilot at work. Open: how does Copilot CLI authenticate at work, and is a separate Copilot-only token possible? May company repos be processed only with Copilot? A private Claude Code on company code is probably not allowed; clear it with compliance.
3. **CI at work.** Decided: start with GitLab CI. Open: Jenkins is the CI at work today (a GitLab CI migration is planned), and until then results may reach GitLab only as external commit statuses, which the first `gitlab-ci` provider does not read. Are pipelines already enabled on the target projects? Do the pipeline rules cover `chore/*` branches and merge request pipelines?
4. **Judge.** If the judge is used on company code: which provider/model is approved for diffs?
5. **`pwsh` and Bash.** Is `pwsh` available where company runs happen, and which shell do the verifier scripts expect?
6. **Open source.** If the project is published: license, and a short README with the host and agent traits and the "Origin and notice" section (verbatim).
7. **Name.** `fleetify` (nod to Spotify's "Fleet Management"). A web search found no open-source project of that name, but found `Fleetify.id` (an Indonesian SaaS for vehicle maintenance) and an API path `/fleetify/` at NextBillion.ai. crates.io availability is checked in M0; PyPI (for `fleetify-recipes`), npm, GitHub names and trademark law are not checked. Check before publishing. Until then the name is only text in code and CLI and easy to change.
8. **Permissions per agent.** Which command-line options for allowed actions and allowed directories does each CLI offer (Copilot CLI is the reference, Claude Code, opencode to be checked), and can the adapters express the agent-neutral `Permissions` with them? Can the verifier's child processes (package caches, scanner) still reach what they need inside those permissions? Test with the canary check.
9. **`git clone --local` on Windows.** Whether hard links are used on NTFS and whether a hard-linked object store causes problems when the workspace is deleted. Checked in M4.
10. **Binary distribution at work.** Can an unsigned binary be run on the work Windows machine, or does it need signing or an allowlisting ticket? Decides whether a release pipeline with signing is needed before M15.

## Working agreement for Claude Code

- Everything in English: identifiers, comments, commit messages, docs, user-facing text.
- One branch per milestone (`m01-command-runner`, …), small commits, tests first for guards, credential matching, command building, report building and idempotency. A milestone is done when its acceptance criteria pass on both OS in CI and `clippy -D warnings` is clean.
- Check each agent CLI's and each host API's current documentation before relying on flags or endpoints; note the documentation URL and date in the adapter's module doc comment.
- Ask before every step marked `[ASK]`: anything that touches real repos, real agents or a merge.
- Never write secrets into logs, events, task files, state files, PR bodies or the report.
- Every new failure path must produce a `failure` block in the report with a non-empty message; a test asserts it.
- When a design question is not answered by this plan, choose the most conservative option, note it in `DECISIONS.md` and continue; do not block on it.

## Todo

- **Config and data directories.** Decide whether configuration (`config.toml`, `credentials.toml`) moves to `~/.config/fleetify/` (XDG) and run data stays under `~/.fleetify/` or moves to `~/.local/share/fleetify/`. Windows and macOS need their own system locations (e.g. via the `directories` crate). Not implemented yet; the defaults above stay until this is decided and recorded in `DECISIONS.md`.
