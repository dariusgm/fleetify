# Agent layer (Claude Code, opencode, GitHub Copilot CLI)

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

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

