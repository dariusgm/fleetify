# Decisions

> References to `PLAN.md` point to the plan before it was dissolved (2026-10-23). Its design is in `docs/design/`, its open items in `tasks/`.

Design questions the plan does not answer. Each entry names the milestone.

## M0 — skeleton

- **Library + binary crate.** `src/lib.rs` holds the modules so integration
  tests can reach them; `src/main.rs` only parses arguments.
- **Tool log location.** Until run folders exist (M4), `fleetify.log` lives in
  the working directory itself. From M4 on it moves into the run folder as
  PLAN.md specifies.
- **Panic hook exits directly** with code 4 instead of unwinding. Child
  process trees are terminated by the runner (M1); the hook will call into it
  once children exist.
- **Working directory without extra dependency.** `HOME` (or `USERPROFILE`)
  plus `.fleetify`; no `dirs` crate.
- **Debug subcommand.** `debug-panic` is hidden from help and exists only to
  exercise the crash path (exit code 4) in tests.
- **Crate name check.** `fleetify` and `fleetify-recipes` returned 404 on
  crates.io when checked. Only crates.io was checked; see PLAN.md open
  question 7 for the other registries.
- **Edition 2021, no pinned MSRV.** A `rust-version` was set in M0 but dropped
  in M2: current `toml` needs a newer rustc and the warning made the pin
  meaningless. Revisit when there is a release target that needs one.

## M1 — command runner and event stream

- **Process groups via `command-group` 5.** Gives one call for the Unix
  process group and the Windows Job Object. Chosen over hand-written FFI.
  **Windows is not verified** on the machine where M1 was built; the Windows
  path needs a run on the Windows CI matrix before M1 counts as done.
- **Synchronous runner, no tokio yet.** Output is read by one thread per pipe
  and multiplexed through a channel. tokio arrives with `--parallel` (M7).
- **Single-pass placeholder substitution.** A substituted value is never
  scanned again, so a repo name containing `{tools}` stays literal. Unknown
  placeholders are errors; other braces (`{}`, `{a b}`) stay literal.
- **Timeout kills the group, then reports `timed_out`.** If the direct child
  exits normally but pipes stay open (a grandchild escaped the group), the
  group is killed and the pipes are given a 2 s grace period.
- **Stray children after a normal exit** are killed only when they still hold
  the output pipes. Daemons that closed their pipes are left alone.
- **Environment.** `EnvPolicy::Inherit` is the default. `EnvPolicy::minimal()`
  is the allowlist from `BASE_ENV`; the agent adapters (M9) use it.
- **Redaction** is exact-value replacement only for now. Token rules and
  config-driven secrets come with M2.
- **Retention.** Every output line is emitted as an event. At most 10,000 lines
  are kept in memory per command (`lines_dropped` counts the rest).
- **Known limitation.** Output lines are read with `read_until(b'\n')`, so a
  process that writes megabytes without a newline grows one buffer. A line cap
  is planned before real agents run (M9).
- **Event bus.** A plain fan-out of `mpsc` senders with dropped subscribers
  removed on emit. Enough for stage 1; not a general broadcast channel.
- **Plain renderer** lives in `ui.rs` (M1 scope). The live view is in `live.rs` (M13).

## M2 — config, repos and credentials

- **`credentials.rs` is a separate module.** The plan lists `config` for both
  files. Tokens sit in their own module so the `Secret` type and its redacting
  `Debug` stay in one place.
- **Glob semantics.** `*` matches any run of characters, including `/`. So
  `gitlab.example.com/team-a/*` also covers nested subgroups. Only `*` is special.
- **Credential patterns must start with a dotted host name**
  (`github.com/*`, not `team-a/*`). This catches a rule that would silently
  match nothing.
- **Open credentials file is an error on POSIX** (any group or other bit set),
  not a warning. On Windows the ACL is not checked; a warning says so.
- **Missing files.** Missing `config.toml` means defaults. Missing
  `credentials.toml` means no rules, so every repo fails with the no-rule error
  when a token is needed.
- **`config.toml` rejects unknown keys** (`deny_unknown_fields`). A token typed
  into the wrong file fails at load time rather than being read silently.
- **Where the files live.** The loaders take explicit paths. Deciding the
  default location and how `workdir` from config interacts with `--workdir` is
  left to the CLI wiring (M4); `--workdir` wins over `workdir` in the file.
- **`repos.txt` rules.** Only full-line comments (`#` first). Duplicates are
  rejected case-insensitively because folders mirror host and path. Every bad
  line is reported with its number. `http://` is rejected; `https` and `ssh` are
  accepted. Credentials embedded in an HTTPS URL are rejected with a message
  that does not echo them. Ports are kept in the clone URL only.
- **Path validation.** Segments may contain `[A-Za-z0-9._-]` only, and `.`/`..`
  are rejected. Paths become folder names, so this is path-traversal protection.
- **Redaction in logs.** `logging::set_redactor` installs the secrets globally
  after credentials load, because the log is opened before config. The log
  writer redacts each write. This assumes tracing writes each event in one
  call, which holds for the JSON formatter; a test covers the writer.
- **Not yet wired.** The runner is not yet built with the credentials' redactor;
  that happens when a run loads credentials (M4). The log redaction is already
  live once `set_redactor` is called.

## M3 — recipe format

- **`applies_to` semantics** (the plan's comment is ambiguous, so the reading
  is recorded here). Every listed condition must hold. Within `host_type` and
  `path_glob` any listed value may match. `has_file` needs *all* listed files,
  `has_file_any` needs *any*, and `not_has_file` needs *none*. The plan lists
  both `has_file` and `has_file_any`, which would be redundant under "any" for
  both. `path_glob` matches the repo path (`group/sub/name`), not the host.
- **Commands** are an argv list, or `posix` and/or `windows`. A command with
  only one OS variant is refused, because a run on the other OS would then
  fail or be skipped silently.
- **`commit_message` is an optional field** that defaults to `pr_title`. The
  plan's skip-CI check names a commit message, and the recipe has nowhere else
  to put one.
- **Skip-CI markers** are matched case-insensitively against the branch, the
  PR title and the commit message. The list is in `recipe.rs` and includes
  `[skip ci]`, `[ci skip]`, `[skip-ci]`, `[no ci]` and `skip-ci`.
- **Branch names** are checked against the git ref rules that matter here, in
  pure Rust. Once git is required at run time (M4), `git check-ref-format` can
  replace the approximation.
- **Instruction placeholders** (`{repo.name}` and friends) are not checked at
  validate time yet. Rendering happens in M7, which is when unknown names
  should fail.
- **Verify is optional.** Without a `[verify]` table no outer verifier runs.
  Pure `prepare` recipes need no verifier at all.
- **`validate` exit code is 3** (refused before starting), matching `run`.
  `validate --agent NAME` lets a recipe with instructions and no `agent` pass
  validation, as the plan's rule allows.
- **Schema** is `schema = 1` and is exported with schemars. `fleetify schema`
  prints it.
- **Defaults**: `ci.wait` is false, `ci.provider` is `auto`, `ci.merge` is `off`,
  `limits` 60/10 minutes, guards 50 files / 2000 lines / test deletion and binary
  files forbidden. `prepare` defaults to 120 s and `verify` to 300 s.
- **Example recipe** `examples/recipes/enable-gitlab-ci` follows the plan's
  example with two changes. It has no `[prepare]` step, because it does no
  useful work there. And its verifier only checks that `.gitlab-ci.yml`
  exists, which keeps it honest as an example.
- **Issues are de-duplicated.** One problem is reported once even when it
  shows up for both OSes.

## M4 — cache clones, workspaces and dry run

- **Credentials reach git through a `GIT_ASKPASS` file, not a pipe.** For one git
  call the token goes into a `0600` file under `<workdir>/tmp`, named in the
  environment variable `FLEETIFY_ASKPASS_FILE`. The same binary answers git's
  prompts when that variable is set (checked in `main` before argument
  parsing). The file is deleted when the call ends. The plan's pipe or
  file-descriptor design was replaced because it does not work portably. The
  token never appears on a command line, in an event or in a log.
- **Git's own credential helpers are switched off** (`-c credential.helper=`).
  A user's `gh` or Git Credential Manager cannot supply a different token behind
  fleetify's back.
- **Credential rules are required for every host access**, public repos
  included, as the plan says. The public test repo therefore needs a matching
  rule. Git never sends the token for a public clone, so the rule's value is
  unused there.
- **HTTPS username** is `x-access-token` for GitHub and `oauth2` for GitLab.
- **Askpass path not yet exercised end to end.** The only authenticated remote
  available is private, and the fake HTTP host comes with M6. Unit tests cover
  the prompt choice and the file lifecycle.
- **Cache clones** live under `<workdir>/clones/<host>/<path>`. The default
  branch comes from `git ls-remote --symref origin HEAD`. The cache is then
  checked out on that branch at the fetched tip and cleaned with `clean -ffdx`.
  Dry runs keep these clones and the second run fetches (verified against
  `dariusgm/git2duck`).
- **Workspaces** are `git clone --local` of the cache at
  `runs/<run>/<host>/<path>/workspace`. The branch starts at the base SHA,
  `origin` is the real URL, and `remote.origin.pushurl` is
  `invalid://fleetify-push-disabled`. `.agent/` is added to
  `.git/info/exclude`. Tests check that status and diff work and that a push from
  inside fails.
- **Open question 9 (hard links on NTFS)** is still open. Only Linux was run.
- **Dry-run results**: `dry_run` (applicable, the workspace would be created),
  `skipped` (applies_to unmet, with the unmet conditions), `error`. Only `error`
  is unsuccessful, so the run exits 1 when any repo errors.
- **Events and logs.** A dry run writes `runs/<run>/events.jsonl` and logs to
  `runs/<run>/fleetify.log`. Every other command logs to `<workdir>/fleetify.log`.
  `RepoFinished` was added to the event enum.
- **Run ids** are `<recipe>-<YYYY-MM-DD>` in UTC. Dates are computed without a
  time crate.
- **`--only`** matches a path suffix. A suffix that matches no repo refuses the
  run (exit 3), so a typo cannot silently skip everything.
- **`clean`** deletes `workspace/` directories under a run and keeps clones and
  logs. Without `--run` it picks the most recently modified run.
- **Config location.** `config.toml` and `credentials.toml` are read from the
  working directory. The `workdir` key in config.toml was removed (unknown keys
  are rejected), because a config file cannot tell you where it lives.
- **Not in M4.** The TLS self-test needs the HTTP client, so it moves to M6.
  Parallel execution is M7. Non-dry `run` still refuses with exit 1.
- **GitLab.** `gitlab:group/sub/name` and clone URLs on gitlab.com go through
  the same git path, so subgroup paths need no special handling. The gitlab
  acceptance (a repo with a subgroup and a `gitlab.com` rule) is not run yet.
  It needs your token and a repo.

## M5 — event consumers, report and status

- **One event consumer thread** (`state::spawn_recorder`) writes `events.jsonl`,
  per-repo `state.json` and `output.log`. The orchestrator only emits events.
- **`state.json` is replaced atomically** (temp file, then rename) on every
  phase change, output line and result, so `fleetify status` can read it at any
  time.
- **`output.log` replaces the plan's `agent.log`** until agents exist (M9). It
  holds every command line (`$ argv`), every output line and the final result.
  `fleetify logs <repo>` tails it, and `-f` follows it.
- **`results.jsonl`**: one line per repo per invocation, last line per repo wins.
  The report of an invocation is built from that invocation's results, so a
  re-run with `--only` changes only the repos it touched in the report.
- **`report.json` shape** follows the plan (`schema`, `run_id`, `recipe`,
  `started`, `finished`, `exit_code`, `summary`, `repos`). Each repo has
  `result`, `successful`, `detail`, `rule`, `change_request`, `pushed_sha` and
  `failure`. The plan's `phases` map is not there yet, because nothing records
  phase durations until M7. `change_request` and `pushed_sha` are always null
  until M7.
- **`dry_run` counts as successful** because a dry run has no failures, only plans. The
  plan's successful list does not mention it.
- **Failure blocks** are built for every unsuccessful repo. A missing message is
  filled in from the detail, and the report never has an empty one. Messages are
  limited to 2000 characters, output tails to 40 lines of 300 characters, and every
  text is redacted with the credentials' secrets.
- **`run.json`** holds run-level facts (`run_id`, `recipe`, `started`,
  `finished`). `finished` stays empty until the run ends. `report` rebuilds
  the report from `run.json` and `results.jsonl` when `report.json` is missing.
- **Ctrl-C.** The first Ctrl-C sets a process-wide flag. Running commands are
  killed as a whole tree (`Outcome::Interrupted`), the repo in progress becomes
  `interrupted` with its output tail, the repos not yet started become
  `interrupted` too, and the report is still written. The exit code is 1. A second
  Ctrl-C does not force an exit yet.
- **`status --watch`** redraws every 2 s until the run is marked finished.
  `logs -f` polls every 500 ms.
- **Not in M5 (deferred to M7).** The plain renderer that prints every event
  as a prefixed line, and the periodic summary line. The final table is printed.
  Dry runs are short enough that the periodic summary has no value yet.
- **Time** is UTC, RFC 3339, from the new `clock` module.
- **Dev dependency.** `jsonschema` with `default-features = false`, to validate
  the report against its schema. With default features it pulled in an HTTP
  stack and a C library that cannot be cross-built for Windows here.

## M6 — host layer

- **HTTP client: reqwest 0.13, blocking, `rustls` feature.** Its rustls
  verifier reads the OS certificate store, which is what the internal CA
  needs. native-tls was the other option: it would add OpenSSL as a runtime
  dependency on Linux. `aws-lc-sys` is a C build. It builds on Linux and is
  expected to build on the Windows CI runner. Cross-checking the Windows target
  from this Linux machine is not possible without a mingw toolchain.
- **Proxies.** reqwest honours `HTTPS_PROXY` by default. Not tested here (no
  proxy in this environment).
- **Token placement.** GitHub gets `Authorization: Bearer`, GitLab gets
  `PRIVATE-TOKEN`. Tests check that a token never appears in a URL or body.
- **API bases.** github.com uses `api.github.com`, other GitHub hosts
  `/api/v3`, and GitLab hosts `/api/v4`. There is no config override yet.
- **Read-access probe.** One `GET` per repo before the clone: 200 is ok,
  401/403/404 map to named results, and any other status is an error with the
  host's message. A rejected token therefore fails in the dry run without a
  clone. The dry run's detail reads `read access ok; …`.
- **TLS self-test.** One HTTPS request per host (on the API base) before the
  repos are processed. A certificate the OS does not trust stops the run with
  exit 3 and says to install the internal CA. Other failures (DNS, timeout)
  only print a warning, because the repo will then fail with a clearer message.
  The classification is unit-tested. The live check ran against api.github.com.
- **Open change requests.** Only `state=open` counts. The client also filters
  the result locally, so a closed or merged item the host returns is ignored.
- **Merge guard.** `merge` sends the expected head SHA. The host refuses a
  mismatch (409 on both hosts), and a refusal becomes `MergeOutcome::Blocked`
  with the host's reason. An unexpected status is an error, not a block.
- **Not called yet.** `default_branch`, `find_change_request`,
  `create_change_request` and `merge` have no caller until the pipeline (M7).
  The cache keeps using `ls-remote` for the default branch.
- **Tests.** A small fake HTTP server in `tests/support` records requests. It
  covers every status mapping, subgroup encoding, open and closed filtering,
  creation, the SHA guard and refusals. The real host was exercised with a
  placeholder token (401 shown) and with the `gh` token (read access ok). The
  token was absent from every file of the run.

## M7 — the no-agent pipeline

- **One flow for dry runs and real runs.** `run::execute` does the setup, the
  worker pool, the results log and the report. A dry run and a real run differ
  only in the per-repo step (`dry_run_repo` or `pipeline::process_repo`).
- **Workers are std threads, not tokio.** Each step is a blocking git or HTTP
  call, so scoped threads do the job without an async runtime. This is a
  deliberate deviation from the plan's `tokio` pool.
- **Per-repo order.** Probe read access, cache, `applies_to`, idempotency
  (an open change request means `already_done`), then a remote branch without
  a change request means a resume (open the change request only). Otherwise:
  workspace, prepare, `git add -A`, no changes means `no_changes`, verifier,
  guards on the staged diff, one commit, push through the explicit URL, and
  the change request.
- **Workspace restart.** A workspace left by an earlier attempt of the same run
  is deleted and rebuilt, as the plan's run flow says.
- **Placeholders.** `{repo}` is the workspace path. `{tools}` is the trusted copy
  of the recipe's `tools` directory. With no agent there is no untrusted copy yet.
- **Verifier output.** Lines matching `error_patterns` are kept. If none match,
  the last `tail_lines` lines are kept.
- **Guards** (`guards.rs`, pure and tested): file and line limits, binary files,
  test deletion (any deleted path containing "test"), deny and allow path globs,
  and `deny_added_patterns` on added lines. Renames are not detected: a rename
  is a delete plus an add, so test-deletion stays strict. `diff.patch` is kept
  for every guard decision.
- **Commit identity.** The repo's own `user.email` is used when set. Otherwise
  the commit is attributed to `fleetify <fleetify@localhost>`. Without this, a CI
  runner with no git identity would fail every commit.
- **Push.** Through the explicit clone URL with the askpass token. The
  workspace's disabled `pushurl` is left as it is.
- **Refused for now.** `--merge` (M11) and any `--waves` other than the default
  single wave (M8). Agent runs are M9 (see below).
  `--continue-if-green` is also M8.
- **Results.** `pr_open`, `already_done`, `no_changes`, `skipped`, `verify_failed`,
  `guard_violation`, `error` (prepare failed or host error), `interrupted`.
- **Tests.** `tests/pipeline.rs` runs the real flow: a bare local remote, a
  fake host and real git and shell steps (Unix only). It covers the happy
  path, a second run that finds the open PR, a failing verifier, a guard
  violation and a no-change run. Each checks the remote state, not only the
  status.

## M9 — the claude-code agent

- **Scope.** Only `claude-code` runs. `opencode` and `copilot` are still refused
  with a message; they come with M10.
- **Where it runs.** After the prepare step, before the commit check. The agent
  works in the prepared workspace. The verifier and the guards then judge what it
  changed, as for every other step. A failing agent ends the repo at `agent`.
- **Task and attachments.** The instruction texts are joined into
  `.agent/task.md`. Attachments are copied to `.agent/attachments/<file name>`.
  `.agent/` is in `.git/info/exclude`, so it never reaches a commit.
- **Tools.** Baseline `Read Edit Write Glob Grep`. `Bash(<cmd>:*)` only for a
  command in both `needs.commands` and `config.toml` `[permissions] commands`.
  Web tools need `needs.web` and `permissions.web`. A recipe need that
  `config.toml` does not grant refuses the whole run before any repo is touched.
- **Blanket permission.** `[permissions.agent.claude-code] unrestricted = true`
  adds `--dangerously-skip-permissions`. It is the user's own choice.
- **Environment.** The base allowlist plus `ANTHROPIC_API_KEY` and
  `CLAUDE_CONFIG_DIR`. Nothing else is inherited.
- **Timeout.** `limits.agent_timeout_min`, for the whole agent process.
- **Asking for help.** A `blocked.md` left in the workspace ends the repo as
  `needs_input`. Nothing is committed or pushed.
- **Binary.** `claude` from PATH, or `FLEETIFY_CLAUDE_BIN` when set.
- **check-agent.** A canary: the agent writes `inside.txt` in its workspace and
  must not leave `escape.txt` one level above. A pass writes
  `<workdir>/agents/<name>.checked`. With `policy.require_checked_agent`, a run
  is refused until the marker exists, unless `--allow-unchecked-agent` is given.
- **Not verified here.** The `claude` CLI is not installed on the development
  machine. The argv (`-p`, `--allowedTools`, `--add-dir`,
  `--dangerously-skip-permissions`) follows the documented Claude Code flags. The
  integration test uses a fake binary. Run `fleetify check-agent claude-code`
  once on a machine with the real CLI.

## Delivery steps after the push

- `[delivery] open_mr` (default `true`) and `delete_branch` (default `false`)
  were added. The ladder is described in `PLAN.md` under "Delivery ladder".
- `open_mr = false` ends the repo as `pushed`, a success. No change request is
  looked up or created. A branch that already exists on the remote ends as
  `already_done` instead of being pushed over.
- `delete_branch` is only valid with `ci.merge = "when_green"`. Merge still needs
  the CI gate (M11) and is refused until then, so branch deletion cannot run yet.
- Host layer: `delete_branch` for GitHub (`DELETE git/refs/heads/<branch>`) and
  GitLab (`DELETE repository/branches/<branch>`, with `/` encoded). 204 and 404
  both count as deleted; anything else is an error. Tested against the fake host.

## Waves, agent contract and OpenCode (M8, M9, M10)

- **Waves (M8).** `--waves` takes sizes that end in `all` (default `1,all`).
  `--canary-repos` makes the first wave exactly those repos. After each wave the
  run asks `[y/N]` on a terminal, or continues only with `--continue-if-green`
  and only when the whole wave succeeded. Otherwise it stops: the report marks the
  rest as `summary.pending`, the exit code is 2 (1 if something failed), and a
  second run resumes, because finished repos come back as `already_done`. Dry runs
  ignore waves.
- **Agent contract (M9).** `task.md` starts with the fixed rules block (`fleetify
  rules v1`), then the instructions, the attachment paths and the verifier command.
  The recipe's `tools` directory is copied to `.agent/tools/`, which is the copy the
  agent runs. Blocked: `.agent/blocked.md`. Done: `.agent/result.md`, whose text goes
  into the change request body under "Agent notes (written by the agent, not
  reviewed)", redacted and limited to 4000 characters. The earlier root-level
  `blocked.md` is gone.
- **Agent commits (M9).** After the agent run the branch is reset (soft) to the
  base commit, so the diff is the whole change and the orchestrator makes one commit.
- **OpenCode adapter (M10).** The command is `opencode run <prompt>` in the
  workspace. Permissions are passed as inline config through
  `OPENCODE_CONFIG_CONTENT`, built from the same grants as for Claude Code.
  Everything not granted is denied. `question`, `task`, `skill` and `doom_loop`
  are denied, and external directories are denied unless granted. Bash is denied
  except for the read-only git commands and granted command prefixes (`<cmd>` and
  `<cmd> *`). `OPENCODE_DISABLE_PROJECT_CONFIG=1` stops a repository's own
  `opencode.json` from widening the grants. Provider and model stay in the user's
  own opencode config. The binary is `opencode`, or `FLEETIFY_OPENCODE_BIN`.
- **Grants for a verifier script.** A recipe can grant the exact verifier command
  (`needs.commands = ["sh .agent/tools/verify.sh"]`). Patterns are `<cmd>` and
  `<cmd> *`, so no other `sh` script is allowed.

## Agent questions: yes/no and numbered choices

- **Policy.** Nobody answers an agent's question during a run. The rules block in
  `task.md` tells the agent to choose the most conservative option and note its
  assumptions, so a question is a rule violation or a real blocker either way.
- **Detection.** `agents::wants_input` runs on every output line. It matches yes/no
  markers (`[y/n]`, `(y/n)`) and choice prompts: `/n]` or `/n)` markers (`[1/2/n]`),
  and lists such as `Pick option 1, 2 or n` (a digit, ` or n`, and one of option,
  pick, choose, select). A match terminates the process tree; the repo ends as
  `needs_input` with the last output lines in the failure block.
- **Limits.** A question that uses none of these forms is not detected. The idle
  event and the agent timeout still end such a run. Extend the markers here, with a
  unit test, rather than answering prompts.
- **Why not answer.** An answer chosen by fleetify is an unreviewed decision in the
  change. A stop with the question in the report is the safer default.

## CI status is bound to the pushed commit

- **Rule.** The CI gate asks the host for the CI items of exactly one commit: the
  SHA fleetify pushed. It never looks up a branch or a pull request, and it never
  takes "the latest run" of a branch. A later push to the same branch has its own
  items and does not change the answer for this commit.
- **Host check.** `Host::commit_ci` returns the check runs and commit statuses of
  that commit. Every item must name the asked SHA; an answer for another commit is
  refused as an error, not used.
- **Verdict.** No items: `Missing`. Any failure: `Failed`, listing the failed items,
  even while others still run. Otherwise any pending item: `Pending`. All success
  (neutral and skipped check runs count as success): `Passed`. Polling stops at the
  verdict or at the timeout, and a timeout with pending items is `Pending`, which the
  gate maps to `ci_timeout`.
- **Not done yet.** The gate is not wired into the pipeline, so `ci.wait = true` is
  still refused. GitLab returns `Unsupported`, since GitLab CI is skipped for now.
  Paging beyond 100 check runs per commit is not handled.

## Branch names: fleetify prefix

- **Rule.** Every branch fleetify pushes is `<branch_prefix>/<recipe branch>`. The
  recipe keeps its name (`chore/poetry-to-uv`); the prefix is added by fleetify.
- **Default.** `branch_prefix` unset in `config.toml` means `fleetify`. An empty
  string means no prefix. A trailing slash is ignored, and an invalid prefix is
  refused when `config.toml` is loaded.
- **Why.** A branch under the prefix shows at a glance that fleetify created it. It
  also limits any later automatic action (cancelling superseded CI runs, cleanup)
  to branches fleetify owns.
- **Not decided yet.** A commit identity or a commit-message marker for fleetify's
  commits. The prefix does not prove who pushed a commit; the report records the
  pushed SHA for that.

## Merging without a CI gate: per-repository rules

- Merging needs three things: the recipe asks for it (`ci.merge = "when_green"`),
  the run is started with `--merge`, and a `[[merge_policy]]` rule in `config.toml`
  matches the repository and sets `merge_without_ci = true`.
- A rule matches `<host>/<path>` with the same glob as `credentials.toml`. The
  first match decides. A repository without a matching rule is never merged
  without CI: the result is `merge_blocked`, and the detail names the repository.
- `delete_branch` in a rule is only a permission. The branch is deleted only when
  the recipe also asks for it (`delivery.delete_branch`). The recipe requests and
  the config grants, so a shared recipe cannot grant itself anything.
- The merge passes the pushed commit as the expected head. The host refuses a
  different head, and the result is `merge_blocked`.
- The current config grants all personal repositories (`github.com/dariusgm/*`).
  The rule set is meant to be narrowed per repository before corporate use.

## Status against PLAN.md (revision 3)

The plan is in `PLAN.md`. This list records what the code does today, milestone
by milestone, so that gaps are visible in the repo and not only in chat.

- **M0 skeleton: done.** Matrix CI (Windows and Linux), rustfmt, clippy with
  `-D warnings`, release builds. The panic hook and exit code 4 are wired.
- **M1 runner and events: done.** Process-tree termination is tested on Unix.
  Windows Job Objects are not verified.
- **M2 config, repos, credentials: done.** Redaction and the `GIT_ASKPASS` path
  are in place.
- **M3 recipe format: done.** `ci.wait = true` is refused until M11, so a
  gate is never skipped silently.
- **M4 cache, workspace, dry run: done.** Open: the NTFS hard-link question (plan
  question 9) is not checked.
- **M5 event consumers: mostly done.** State, events, report, status, logs and
  Ctrl-C are present. The `--format json` output and `permissions_effective`
  event are not built.
- **M6 host layer: done.** GitHub and GitLab over REST, tested against a fake host.
- **M7 no-agent pipeline: done.** Parallelism uses threads, not `tokio` as the plan
  says. Revisit with M11, where the CI gate is planned as a separate task.
- **M8 waves: done.** See "Waves, agent contract and OpenCode" above.
- **M9 agent runner: done.** Acceptance points and where they are covered:
  - Task assembly with the rules block, instructions, attachments and verifier line
    (`tests/agent.rs`).
  - Allowlisted environment for claude-code (`agents` unit test) and for opencode
    (`OPENCODE_CONFIG_CONTENT`, unit test).
  - A recipe whose `needs` exceed `config.toml` is refused before any repo is touched,
    with exit 3 and the entry named (`tests/cli_reports.rs`). A dry run shows the same
    gap as a warning and continues.
  - Edit, self-commit, blocked (`needs_input`), crash, forbidden change
    (`guard_violation`) and a yes/no question (`needs_input`) (`tests/agent.rs`).
  - A hanging command is terminated with its process tree on Unix (`tests/runner.rs`).
    Not verified on Windows.
  - `permissions_effective` and `command_idle` events; `agent_timeout` result.
  - Run header warning when an agent is `unrestricted`; output lines are capped per
    command (`lines_dropped`).
- **Decisions that close the open M9 points.**
  - *Inner-loop verifier.* The agent runs the verifier command from `task.md` against
    its copy under `.agent/tools/`. That is the inner loop. The outer gate runs the
    trusted original after the agent exits, so a tampered copy cannot pass the gate.
    No hard enforcement of "run it after each change" is possible; the rules block asks
    for it.
  - *Questions without a marker.* Only the markers in "Agent questions" stop a run. A
    heuristic on silence or on a trailing `?` would stop agents that are only writing
    prose, so it is not added.
  - *Writes outside the workspace.* Claude Code is confined by `Edit` path rules, set
    by fleetify in the tool list (`Edit(//<workspace>/**)` and the granted directories).
    An earlier version assumed the CLI confined writes by itself; it did not. A real
    `check-agent claude-code` run found an escape to the parent directory, and that is
    why the rules exist. `check-agent` now runs two prompts, one that must write inside
    and one that must not write outside. Reads are not limited by path in Claude Code,
    so the agent can read files outside the workspace. fleetify does not watch the file
    system during a run.
- **M10 opencode and check-agent: mostly done.** The opencode adapter is in, with
  the grants as inline permission config. `check-agent` runs for both agents (the
  canary is simpler than the plan: no decoy marker, no cache per agent version). The
  run header prints the effective grants of the agent (`describe_grants`). A dry run
  reports missing grants as warnings and continues; a real run refuses to start.
  Open: the level-3 smoke test with opencode and llama.cpp on a throwaway GitHub repo,
  which needs an explicit go-ahead.
- **M11 CI gate and merge: mostly done (GitHub).** `ci.wait = true` is implemented for
  GitHub. The gate judges the commit fleetify pushed (or, for `already_done` with
  `--merge`, the head of the existing change request), through `Host::commit_ci` and
  `ci::poll`. Results: `ci_passed`, `ci_failed`, `ci_missing`, `ci_timeout`,
  `ci_blocked` (GitLab, skipped for now). A merge runs only after `ci_passed`, with the
  pushed or checked SHA as the expected head, and with the existing three keys.
  Deliberate change from PLAN.md: the plan's M11 says `wait = true` on GitHub is a
  validation error; the GitHub gate is wanted now, so that error is gone. Missing: the
  paging of more than 100 check runs per commit, and a test for the timeout (the
  verdict logic is covered by unit tests).
- **M12 Python authoring package: not started.**
- **M13 live view: done, not yet checked in a real terminal.** `--ui` on a terminal
  draws a table of repos (phase or result, idle time) and the last output lines of the
  running repos (`--tail`). Ctrl-C in the view stops running commands. `NO_COLOR`
  removes the colours. Without a terminal, or with `--plain`, every event is one line on
  stderr; stdout keeps the final table. The view only reads the event stream, so the
  report and the state files are the same with and without it.
  Open: the manual check on Windows Terminal and a Linux terminal (plan M13 acceptance).
- **M14 Claude Code adapter: done for the smoke test.** The canary passes on repeated
  real runs, and the file-edit confinement is in place (`Edit` path rules). The `[ASK]`
  smoke test ran on a private throwaway repo (`dariusgm/fleetify-m14-smoke`, one unused
  import): the agent removed it, the verifier passed, and PR #1 was opened. Only
  `hello.py` changed, and no token appears in any run file. `require_checked_agent` is
  true by default. Open: the full level-5 lint test with the complete gate chain; the
  throwaway repo can be deleted when no longer needed.
- **M15 Copilot CLI adapter: not started.**
- **M16 LLM judge: not started.** `judge` is accepted by the parser and not used.
- **M17 real trial: not started.**

Declared but unused: none after the MVP cleanup of 2026-10-22 (see below).

## Example run — Python 3.15 on dariusgm/blog (private)

- **Recipe.** `examples/recipes/python-315-update`. Prepare sets
  `Dockerfile` to `python:3.15.0` and `pyproject.toml` to `python = "3.15.*"`,
  then runs `poetry lock` (Poetry 2.5.1 via `uvx`). Verify checks the pins and
  `poetry check --lock`, installs the project under Python 3.15.0 and runs
  `make html`. Guards allow only `Dockerfile`, `pyproject.toml` and `poetry.lock`.
- **What changes in the lock.** Only the header (Poetry version, because the
  recipe pins 2.5.1), `python-versions` and `content-hash`. Every package pin is
  unchanged (checked by diffing the lock).
- **Result.** PR #12 (`chore/python-3.15` → `main`) on dariusgm/blog: 3 files,
  +5 −5. A second run reports `already_done` and opens no duplicate.
- **Bug found by the first real run.** A relative recipe path made the prepare
  step fail, because `{tools}` was resolved against the current directory and
  the step runs in the workspace. Fixed: the tools path is made absolute before
  any step runs. The failed run pushed nothing (verified: no branch, no PR).
  Tests use absolute recipe paths, so this is not covered by them yet.
- **Requirements of the recipe.** A POSIX shell (`sh`) and `uv` 0.13 or newer
  (older builds do not know Python 3.15). On Windows the recipe fails with
  "sh not found" as an error result.
- **Credentials.** `~/.fleetify/credentials.toml` (mode 600) holds the `gh`
  token for the `dariusgm/*` rule, for this and later runs.

## GitLab CI gate (M11, GitLab part)

- **Implemented.** `commit_ci` for GitLab reads `GET /projects/:id/pipelines?sha=`,
  so branch and merge request pipelines of the pushed commit both count. The gate
  therefore works for `ci.wait = true` and `--merge` on gitlab.com the same way as on
  GitHub. Pipelines that name another commit are a parse error, never a verdict.
- **Skipped pipelines** are left out. If only skipped ones exist, the result is
  `ci_missing`, as PLAN.md says.
- **Deviation: `manual` is pending, not `ci_blocked`.** `CiState` has no blocked
  state yet. A pipeline waiting for a manual job now ends as `ci_timeout` after
  `timeout_min`. Both results are non-successful, so nothing merges. A `Blocked`
  state would give the planned `ci_blocked` result; not done yet.
- **Not covered:** `allow_failure` jobs (the pipeline status is used as is), external
  commit statuses, and the failure details of `ci_failed` for GitLab.

## Outage handling (M11a)

- **Implemented.** A transient host error (no answer, 429, 5xx) is retried by the CI
  poll with exponential back-off (`ci::backoff`, capped at 5 min) until the CI budget
  is spent. Each retry is a `host_retry` event. A failure that outlasts the budget ends
  the repo as `host_unreachable` with a failure block; other errors stay `error`.
- **Implemented.** Before a merge the change request is read again. A merged change
  request is `merged`, a closed one is `merge_blocked`, an unreachable host is
  `host_unreachable` and nothing is merged. A run that finds only a merged change request
  for its branch ends as `already_done` and opens no second one (a merge whose response
  was lost). New host method `latest_change_request` (any state, newest first).
- **Implemented.** A push that fails is checked against the remote branch before anything
  else. Same commit: the earlier push counts. Different commit: `error`, nothing is
  overwritten. Remote not readable: `host_unreachable`.
- **Implemented.** An unreadable remote while refreshing the cache is `host_unreachable`
  (checked with `ls-remote`), not `error`.
- **Not done, deviation from PLAN.md:**
  - `Retry-After` is not parsed; 429 uses the same back-off as 5xx.
  - Reads outside the CI poll (default branch, change request lookups, change request
    creation) are not retried. A transient error there ends the repo as
    `host_unreachable` at once; a re-run continues. Plan rule 1 asks for retries here too.
  - Clone and fetch are not retried. The unreachable case is found with one `ls-remote`
    afterwards, so the repo ends as `host_unreachable` without a second attempt.
  - The push-divergence refusal has no test: a branch that exists before the run never
    reaches the push, so the test needs a race. The poll and merge paths are tested.

## Host rate limits (extension of M11a)

- **Implemented in the HTTP client.** Every host call goes through `Http::send`. A
  rate limit within `[rate_limit] max_wait_min` (default 15 min, `0` = never wait) is
  waited out and the request repeated, at most three times. A longer one returns
  `HostError::RateLimited` without waiting, and the repo ends as `rate_limited`
  (not successful, failure block with the wait and reason).
- **Recognised as a rate limit:** HTTP 429, and HTTP 403 whose body says "rate limit"
  (GitHub's secondary limits). Wait: `Retry-After` in seconds, else the reset time, else
  60 s.
- **Quota exhausted on a successful reply** (`x-ratelimit-remaining` or
  `ratelimit-remaining` is `0`): the reply stands, and the host is held back until the
  reset for the next requests.
- **Shared per host.** All repos of one host use one client, so one hold-back applies to
  all of them.
- **Events.** `rate_limited` is emitted with host, wait and reason for each wait that is
  taken. The client keeps only a weak handle on the event bus; a strong one kept the
  event stream open and hung the run (found by the agent test).
- **Checked against the docs (2026-10-21):**
  - GitHub (docs.github.com, REST rate limits): `x-ratelimit-remaining`,
    `x-ratelimit-reset` (UTC epoch seconds), `retry-after` (secondary limits, when
    present). Primary limit: 403 or 429 with remaining `0`. Secondary limit: 403 or 429;
    the exact message text is not documented, so the body check is a heuristic. Wait order
    as implemented: `retry-after`, then reset, then 60 s.
  - GitLab (docs.gitlab.com, user and IP rate limits; GitLab.com rate limits): throttled
    requests return 429. `RateLimit-Remaining` and `RateLimit-Limit` are on all responses.
    `RateLimit-Reset` (Unix time) is documented on responses; `RateLimit-ResetTime` and
    `Retry-After` (seconds) on 429 only. A 403 without headers (failed-auth ban) is not a
    rate limit and stays `Forbidden`.
  - The GitLab REST page itself does not document the headers; the admin and GitLab.com
    rate-limit pages do.
- **Not done:** `Retry-After` as an HTTP date (only seconds are read); a proactive
  per-minute limit; agent rate limits (planned in the next branch).

## Copilot CLI adapter (M15a)

- **Version and account.** Copilot CLI 1.0.101 (`copilot --version`) on Copilot Free, on
  this machine. `check-agent copilot` runs the canary against the real CLI.
- **Invocation.** `copilot -p <prompt> -s --no-ask-user`, text output (not JSON), so the
  output lines keep working with `wants_input`. Documented in the programmatic reference.
- **Observed on 2026-10-21, real CLI:**
  - `-p` with a tool that is not granted: the agent says the write was denied and exits 0.
    The exit code does not show a denied permission; the canary checks the files instead.
  - `--allow-tool=write` without a path pattern writes anywhere, with or without `--add-dir`.
    So the adapter never emits a bare `write`.
  - `--allow-tool=write(<dir>/*)` allows writes directly inside `<dir>`, but not in
    subdirectories, and denies writes outside. `--allow-tool=write(<dir>/**)` allows writes
    in subdirectories too and still denies writes outside. The adapter uses `**`.
  - Repeated `--allow-tool` flags apply together (two directories, both writable).
  - Shell grants match the executable only. `shell(sh:*)` ran `sh .agent/tools/verify.sh`;
    `shell(sh .agent/tools/verify.sh:*)` and `shell(sh .agent/tools/verify.sh)` were denied.
    So a command grant is `shell(<first word>:*)`, which is broader than the recipe's command.
  - With the `**` write grant and no shell grant, the agent created the missing
    directories and the file. Creating directories needs no shell grant.
- **Environment.** Only the base allowlist; no Claude or opencode variables and no host
  token. Copilot's login comes from its own configuration.
- **Not verified:** whether a denied `-p` run prints a recognisable message that could
  become a `needs_input` or `blocked` outcome; the rate-limit behaviour of the CLI (planned
  with the agent rate-limit work); `read` grants are not path-limited (same as Claude
  Code's baseline, covered by the canary only through the write test).
- **Tests.** Unit tests for the argument list; `tests/copilot_fake.rs` for the canary logic
  with fake binaries (confined, leaky, silent); `tests/copilot_live.rs`, ignored by default,
  for the real CLI.

## Level-5 run: Copilot CLI on a throwaway GitLab project (M15a acceptance)

- **Repo.** A private, throwaway gitlab.com project with a Ruby 2.7.8 pin and a CI job.
  Recipe `examples/recipes/ruby-34-copilot`, agent `copilot`, default model of the Copilot
  account (no model option yet, see the model-selection story in PLAN.md).
- **First run: `needs_input`, nothing pushed.** Copilot edited both pins but could not run
  the verifier: the shell grant `shell(sh .agent/tools/verify.sh:*)` was denied. Copilot
  matches shell grants on the executable only; the adapter now grants `shell(<first word>:*)`
  (see the Copilot section above). The stop was the intended behaviour: the workspace was
  kept and the repo was not pushed.
- **Second run: `pr_open`.** Merge request !1 on the throwaway project. The diff contains
  exactly `.ruby-version` (3.4) and `.gitlab-ci.yml` (`image: ruby:3.4`). The GitLab pipeline
  on the pushed commit finished `success`. The merge is left to a human.
- **Result.** The level-5 test of M15a passes. Observed: Copilot follows the recipe's
  instruction, the verifier gate and the guards act as the outer check, and a denied grant
  stops the run as `needs_input` instead of pushing.

## Agent rate limits (extension of M11a, agent part)

- **Implemented.** A failed agent run whose output contains a rate-limit phrase ends as
  `rate_limited`, with the first matching line in the failure block. The phrases are in
  `RATE_LIMIT_MARKERS` (`rate limit`, `usage limit`, `too many requests`, `http 429`),
  compared case-insensitively. A timeout with such a line is `rate_limited` too, because the
  message is the cause.
- **Only for failed runs.** A successful run is never reclassified, so a phrase in code the
  agent printed cannot fail a run.
- **Not verified against the real CLIs.** No real Claude Code, Copilot or opencode rate limit
  has been observed yet. The phrases are a starting list; they must be checked in a level-5
  run before relying on them.
- **Not done:** the agent's own wait is not capped by `[rate_limit] max_wait_min`. An agent
  that waits on its limit runs until `agent_timeout_min`. Reading the reset time from the
  agent's message is not implemented.

## Model selection per agent run (story)

- **Implemented.** The model is chosen in this order: `fleetify run --model NAME`, then the
  recipe's `model` key, then `[agents.<name>] model` in config.toml. Nothing is passed when
  none is set, so the agent's own configuration decides, as before.
- **Recipe key is top level.** The story wrote `[agent] model`, but `agent` is already a
  string key in recipe.toml (`agent = "claude-code"`), so `model` sits next to it instead.
- **Recipe model only for the recipe's own agent.** With `--agent opencode` on a recipe that
  names `claude-code`, the recipe's model is not passed to opencode. A model name belongs to
  one agent; a `--model` value is passed to whichever agent runs.
- **Passed through unchanged.** Flags: Claude Code `--model`, Copilot CLI `--model`, opencode
  `run --model` (the `provider/model` form). The name is only checked to be non-empty, free of
  whitespace and not starting with `-`, so it cannot be read as an option. The adapters'
  flags were not run against the real CLIs for every model yet; the level-5 run per model
  is still open.
- **check-agent per model.** The pass marker is keyed by agent and model. Without a model the
  marker name is unchanged (`<agent>.checked`), so earlier passes still count. Version is not
  part of the key yet (the plan asks for it; no version is read today).
- **Visible.** The run header and the `permissions_effective` event name the model.
- **Report.** Each repo's entry in `report.json` carries `model` when the agent ran with one
  (added to every outcome after the agent step, so a failed repo names its model too).

## API usage credits (`api_limit`) and the outage items of M11 and M11a

- **Observed wording.** `claude -p --model fable` on this account (2026-10-21) prints on
  stdout and exits 1: `Fable 5.1 requires usage credits. Switch to another model, or manage
  usage credits at https://claude.ai/settings/usage?...`. stderr stays empty. The Claude CLI
  binary also contains `You don't have usage credits yet.`
- **`api_limit` is its own result.** Phrases in `API_LIMIT_MARKERS` (`usage credits`,
  `credit balance`) end a failed agent run as `api_limit`, not `rate_limited`: waiting does not
  help until the account has credits. It is checked before the rate-limit phrases and is not
  successful. The failure block carries the CLI's sentence.
- **Wait ceiling for agents.** The runner ends an agent that prints a rate-limit line and then
  stays silent for `[rate_limit] max_wait_min` (default 15 min; `0` ends it at once). The run
  then ends as a timeout, and the rate-limit line in its output makes the repo `rate_limited`.
  A rate-limit phrase followed by more output does not start the clock, so code the agent
  prints cannot end its run. Not verified against the real CLIs.
- **Reset time from the agent.** The agent's own sentence (which may contain the reset time)
  is carried in the failure block as printed. fleetify does not parse it: the CLI's reset format
  was not observed in `-p` mode.
- **Reads are retried.** `hosts::retry_read` tries a host read up to three times with back-off
  (2 s, then 4 s) when it gets no answer or a 5xx. It covers the access probe, the lookup of the
  change request, and the latest change request before a merge. Writes are not retried: a create
  that timed out may have happened, and a second try could open a second change request.
- **Cache fetch and clone are retried** the same way (three tries, back-off).
- **`Retry-After` as an HTTP date.** The IMF-fixdate form (`Wed, 21 Oct 2015 07:28:00 GMT`) is
  read, counted from now; seconds still work. Other date formats are ignored.
- **GitHub lists are paged.** Check runs and commit statuses are read page by page (100 per page,
  at most 50 pages), until `total_count` is reached.
- **`ci_blocked`.** A GitLab `manual` job and a GitHub `action_required` check run are blocked,
  not pending. The gate does not wait for them: the repo ends as `ci_blocked` at once, and a
  failed item still wins. Before, `manual` became `ci_timeout` after the full wait.
- **Not done:** a test for the push-divergence refusal (it needs a race between the state check
  and the push); the rate-limit phrases for Claude Code, Copilot and opencode are still a
  starting list, to be confirmed in level-5 runs.

## Authoring package `fleetify-recipes` (M12)

- **Location.** `authoring/` in this repo. It is a separate uv project with no runtime
  dependencies; `jsonschema` is only needed for the schema check (`validate` extra and dev group).
- **Byte-for-byte output.** The writer emits keys in dataclass declaration order, a blank line
  before each table, and LF endings. Quoting is explicit: strings are double-quoted, strings with a
  backslash are literal (single-quoted) automatically, and `Literal` forces single quotes. The
  hand-written example mixes both inside one array, so a pure rule could not reproduce it.
- **Version.** `write()` refuses any `schema` other than 1. The schema check reads the JSON Schema
  from the binary (`fleetify schema`), so it follows the binary that runs the recipe.
- **Scope.** The dataclasses cover the top-level keys and tables of schema 1 that a recipe written by
  hand uses (applies_to, instructions, prepare, verify, needs, delivery, guards, ci, limits).
  `judge` is not mirrored yet; add it when a recipe needs it.
- **Not done:** publishing to PyPI (the plan's name check is still open).

## Level-3 smoke test with opencode (M10, 2026-10-21)

- **Setup.** Private throwaway repo `dariusgm/fleetify-m10-smoke` (Ruby 3.2 in `.ruby-version` and
  `.github/workflows/ci.yml`). Recipe `~/.fleetify/recipes/ruby-34-opencode-github` (not in the repo):
  agent `opencode`, model `openai/gpt-5-nano` passed with `--model`, verifier `tools/verify.sh`, guards
  allow only the two pins. `check-agent opencode --model openai/gpt-5-nano` passed first.
- **Result.** `pr_open`: PR #1 (`fleetify/chore/ruby-3.4-opencode`) changes only `.ruby-version` and
  `.github/workflows/ci.yml`. The gate ran the verifier after the agent and passed. Nothing was merged.
- **Finding: the agent could not run its own verifier.** opencode ran the command as
  `bash -lc 'sh .agent/tools/verify.sh'`. The grant `sh .agent/tools/verify.sh` does not match that
  form, and the rule `bash *` is denied by the generated config. The agent's `result.md` therefore says
  "the verifier would pass", which was not checked. The gate verified the result; the agent did not.
  Fix to decide: grant the wrapped form (`bash -lc *` for that script only) or accept that the agent
  cannot verify itself with opencode. Not changed yet.
- **Invariants.** Held: no push before the gate, only allowed paths changed, no merge, no secret in the
  run files.
- **Still open for M10.** The test ran against OpenAI, not against llama.cpp (the local server was not
  running). The throwaway repo can be deleted after the PR is reviewed.

## M10 re-test: opencode with OpenAI `gpt-5-nano` after the prompt change (2026-10-22)

- **Change since the first test.** The task prompt was adjusted so the agent runs the verifier
  directly, not through a wrapper. The `bash -lc` finding above is no longer reproduced: the output log
  shows `sh .agent/tools/verify.sh .` as the agent's own call.
- **Setup.** Same throwaway repo and guards as above. Recipe copy with branch
  `chore/ruby-3.4-opencode-openai` (so PR #1 stays untouched). `check-agent opencode --model
  openai/gpt-5-nano` passed.
- **Runs.** Four runs with `--agent opencode --model openai/gpt-5-nano`:
  1. `m10-openai-smoke`: `needs_input`, no push. The agent's patch tool resolved `.ruby-version` to a
     doubled path (`workspace/workspace/.ruby-version`), and its fallback `printf` write was denied by
     the grant. Only `.github/workflows/ci.yml` changed. The agent wrote `.agent/blocked.md`.
  2. `m10-openai-smoke-2`: `pr_open`, PR #3. Changes exactly `.ruby-version` (3.2 to 3.4) and
     `.github/workflows/ci.yml`. Verifier and guards passed.
  3. `m10-openai-smoke-3` and 4. `already_done`, pointing at PR #3. No second push or PR.
- **Result.** The combination works end to end: PR #3 was opened and passed the gates. The first
  run ended blocked on the agent's file edit (the doubled path above); the gate and the invariants
  held in every run.

## M14 trial: Claude Code with Haiku updates dependencies on a public icalendar fork (2026-10-22)

- **Setup.** Public fork `dariusgm/icalendar` of `collective/icalendar`. The CI was cut to the fast
  Linux jobs first: documentation, fuzzing, zizmor, agent-scan, PR-quality and the test matrix were
  removed; `tests` runs `uv run --group test pytest -q` on Ubuntu with Python 3.14, `ruff` stays.
- **Fast local loop.** `prepare.sh` (PyPI lookup) about 2 s; `verify.sh` (ruff format, ruff check,
  pytest) about 40 s on this machine. Full suite: 21297 passed, 40 skipped, 584 xfailed.
- **Recipe.** `icalendar-update-deps` (not in the repo, under `~/.fleetify/recipes/`). Agent
  `claude-code`, model `claude-haiku-5-5`, canary passed before the run. Guards allow only
  `pyproject.toml` and `.pre-commit-config.yaml`; `ruff` and `zizmor` stay pinned; no source or test
  edits.
- **Run `icalendar-deps-1`: `ci_passed`, PR #1.** Diff: five version lines in `pyproject.toml`
  (`tzdata`, `typing-extensions`, `towncrier`, `mypy`, `pytz`), 5 insertions and 5 deletions. The
  agent reported that `mypy` and `towncrier` jump a major version and that it did not run `mypy`
  on its own, so its type-check result is unconfirmed.
- **GitHub Actions on the fork, observed on a real repo.**
  - Push to `main` and push to the PR branch did not start any workflow run. The `pull_request`
    trigger did not start a run either.
  - `workflow_dispatch` started runs without problems, on `main` and on the agent branch.
  - The dispatched run attaches its check-run to the commit SHA. The gate found it there and
    returned `ci_passed`. The PR page itself still shows "no checks reported".
  - The cause of the missing push and PR triggers is not known yet.
- **Fleetify observation.** During the CI wait the status request failed repeatedly with
  "cannot reach the host", and the retry counter stayed at `attempt: 1` for about 19 minutes. The
  same URL worked with `curl` and `gh`. The gate only passed once the manual run appeared. Open:
  the transport error chain is not printed, and the attempt counter does not advance.

## MVP cleanup: CI dispatch, `[judge]` removed (2026-10-22)

- **Breaking change: `[judge]` is removed from `recipe.toml`.** It was parsed and validated but never
  used, and M16 (LLM judge) is not started. The Python package never mirrored it, so the two sides now
  agree. A recipe with `[judge]` fails validation with an unknown-key error. The section comes back
  with M16, if the trial shows the deterministic guards are not enough.
- **CI dispatch.** When the host has started no CI for the pushed commit after the wait, fleetify starts
  it: `workflow_dispatch` on GitHub (`ci.dispatch_workflow` in the recipe, for example `tests.yml`), a
  pipeline on GitLab. The wait is `[ci] dispatch_after_s` in `config.toml`, in seconds; unset means 180,
  `0` starts CI at once without asking the host first. The start is an event (`ci_dispatched`) and
  appears in the log and the output. With `0`, a CI that already runs for the commit is started again.
- **Python authoring.** `Ci.dispatch_workflow` is mirrored; every table of the recipe schema now has a
  matching dataclass. The contract snapshot is regenerated.
- **Probe run.** A recipe that changes one file and runs no agent exercises the dispatch path on the
  fork. The first attempt failed on its own baseline (the probe file is absent on `main`), so the recipe
  sets `baseline = false`. Result of `ci-probe-2`: the push created no CI run; the gate waited the
  default 180 s, then fleetify dispatched `tests.yml` on `fleetify/chore/ci-probe`. GitHub created the run at
  18:46:57 UTC, about three minutes after the wait started. The gate found its check-run on the commit and
  returned `ci_passed` for PR #2 (`dariusgm/icalendar`). The status requests failed with the same
  "cannot reach the host" error during the wait; the open issue from the icalendar entry still applies.
  Not yet tested: `dispatch_after_s = 0` in a live run, and GitLab.


## VoiceStudio reference run: lock refresh with Claude Code Haiku (M17 reference, 2026-10-22)

- **Setup.** Public fork `dariusgm/VoiceStudio` of `debpalash/VoiceStudio` (AGPL-3.0, 81k stars,
  Python backend plus Electron, frontend and mobile). Recipe `voicestudio-lock-refresh` (under
  `~/.fleetify/recipes/`, not in the repo): agent `claude-code`, model `claude-haiku-5-5`, guards allow
  only `uv.lock`, verifier `tools/verify.sh` (`uv lock --check`, `uv sync --frozen --all-extras`,
  `pytest tests/test_app_version.py`), CI `dispatch_workflow = "ci.yml"`, `wait = true`.
- **Local timing.** `uv sync --frozen --all-extras` took about 47 s (278 packages). The full pytest
  collection has 10 511 tests and takes about 22 s just to collect, so the full suite stays with CI.
- **Run `voicestudio-lock-1`: `error`, `HTTP 404: Not Found`, exit 1.**
  - The agent ran `uv lock --upgrade`. About 300 packages moved to the newest versions their declared
    ranges allow. The guards and the local verifier passed.
  - The push succeeded. PR #1 (`dariusgm/VoiceStudio`, branch `fleetify/chore/lock-refresh`) is open,
    with 3466 additions and 3435 deletions in `uv.lock` only.
  - The CI wait started. The status requests failed three times with "cannot reach the host", the same
    error as in the icalendar runs. Then the run ended with `HTTP 404`.
  - No `ci_dispatched` event, and `gh run list` shows no workflow run on the fork. The dispatch was
    not logged. Two causes fit the log and are not separated yet: the dispatch to `ci.yml` returned 404,
    or the check-runs lookup for the commit returned 404. The workflow listing of the fork also returns
    404 while Actions are enabled, so the fork's workflow registration is the first thing to check.
- **Finding for fleetify.** An error from a host call should name the endpoint that failed. "HTTP 404:
  Not Found" does not tell the user where to look. Open.
- **Not yet tested here:** whether `ci.yml` dispatches on this fork at all, and whether the agent's
  lock refresh passes the full CI.

## Directories and the remaining Stage 1 work (2026-10-22)

- **Config location changed (breaking, MVP).** `config.toml` is read from `~/.config/fleetify/` (or
  `$XDG_CONFIG_HOME/fleetify/`), with `--config FILE` as override. `credentials.toml` and the working
  directory stay in `~/.fleetify/`. A config left in the old place makes the run refuse with the path to
  move it to, instead of silently using defaults. opencode keeps its configuration in
  `~/.config/opencode/` but its auth and data in `~/.local/share/opencode/`, so the credential location
  follows the decision above, not the opencode layout.
- **Self-hosted GitLab (M15b).** No host name is hardcoded in the code. `api_base` builds
  `https://<host>/api/v4` for any configured GitLab host, and the tests use `gitlab.example.com` only
  for parsing. The HTTP client uses rustls with the platform verifier (the lockfile contains
  `rustls-platform-verifier`), so the internal CA should be trusted from the OS store. Not verified
  against a real self-hosted instance. Open: the internal CA on a Windows machine, and whether
  `HTTPS_PROXY` is honoured (reqwest reads the environment by default; not tested).
- **Postponed:** M12 (authoring package publication, until the core is stable), M16 (LLM judge).
- **Moved to a Windows test package:** the Windows checks of the stage-1 milestones and the manual
  check of the live view in Windows Terminal. They need a Windows machine or a VM.

## Credentials move next to the config (breaking, 2026-10-22)

- **Decision.** `credentials.toml` sits next to `config.toml`, in `~/.config/fleetify/` (or the directory
  of `--config`). It is no longer read from the working directory. The file was moved on this system.
  This follows opencode, which keeps its configuration and its API keys in `~/.config/opencode/`.
- **Refusal.** A run with a config or credentials file left in the old working directory stops and
  names the new path. It does not fall back to defaults.
- **Test isolation.** The CLI integration tests now pass `--config` to a file inside their own
  directory. Before, they read whatever config the developer had, which granted commands and broke the
  expected refusal.
- **M15b closed** for now. Tests on the self-hosted GitLab run on the company laptop. The Windows checks
  are collected under "Windows Support" in PLAN.md.
- **M12 and M16 postponed** (see the previous entry).

## CI fix rounds (stage 2, 2026-10-22)

- **Scope.** The agent gets the end of each failed job's log, nothing else. Default 100 lines or
  1000 characters per job, whichever is smaller (`[ci] log_lines`, `log_chars` in `config.toml`).
  Failed jobs are capped at five; the earliest failure comes first, by start time.
- **Nested pipelines.** GitLab: the pipelines of the commit, then the child pipelines their trigger
  jobs started (bridges), recursively up to five levels. Trigger jobs and jobs allowed to fail are
  left out. GitHub: every workflow run of the commit; reusable workflows appear as jobs of their caller.
- **Redaction.** The failure file goes through the same redactor as every other output.
- **Rounds.** `[ci] fix_rounds` in the recipe, default 1, `0` disables the loop. A fix round is an
  agent run, the verifier, the guards on the whole change against the base commit, one commit on top
  of the branch and a normal push (never forced). A round that changes nothing ends the repo as
  `ci_failed` with that reason.
- **Tests.** `tests/ci_fix.rs`: a failure that a fix round repairs ends `ci_passed`; a failure that
  stays ends `ci_failed` after one round; `fix_rounds = 0` runs no agent. `tests/gitlab_ci_failures.rs`:
  nested pipelines, trigger skipping, allowed failures, the log tail.
- **Not yet covered.** A live run on GitHub or GitLab with a real failing check. The fake hosts do not
  check the workflow runs' permissions or their log retention.


## Remove the config.toml layer (2026-10-23)

Every behaviour setting lives in the recipe. The user's side is one file, `credentials.toml`, which holds the tokens and the grants. The plan was `tasks/remove-config-layer.md`; the file was removed when the change merged.

- **Decision: no `config.toml`.** The binary no longer reads it. A run with `config.toml` next to the credentials file, or in the working directory, refuses and names the way out. `--config` is replaced by `--credentials FILE` (default `~/.config/fleetify/credentials.toml`, or `$XDG_CONFIG_HOME`). Reason: a setting that comes from a file the recipe does not show makes the recipe not reproducible on another machine.
- **Open question 1, hosts: derived from the URL.** The host name comes from the repository line, as before. A host type is needed only for a host beyond `github.com` and `gitlab.com`, and the recipe states it: `[hosts."gitlab.example.com"] type = "gitlab"`. The built-in hosts keep their types, and a recipe that declares one as another type is refused.
- **Open question 2, permissions: the recipe asks, the user grants.** `needs` (commands, web, extra directories, and now `unrestricted`) and `policy` are asks. The grants live in `credentials.toml`: `[permissions]` (`commands`, `web`, `extra_directories`, `unchecked_agents`) and `[permissions.agent.<name>] unrestricted`, plus `[[merge_policy]]`. A run that asks for more than is granted is refused before any repository is touched. Keys and grants go under `credentials.toml` and nowhere else; the recipe never holds a key.
- **Open question 3, defaults: unchanged.** `policy.require_checked_agent = true`, no commands or web granted, `rate_limit.max_wait_min` 15, `ci.dispatch_after_s` 180, `ci.log_lines` 100, `ci.log_chars` 1000, branch prefix `fleetify`, no model.
- **Moved into the recipe:** `[ci]` `dispatch_after_s`, `log_lines`, `log_chars`; `[agents.<name>] model` (the recipe's own `model` still wins, then `--model`); `[rate_limit] max_wait_min`; `[policy] require_checked_agent`; `[hosts]`; top-level `branch_prefix`.
- **Stays in `credentials.toml`:** `[permissions]`, `[[merge_policy]]`, `[[credential]]`. The file is still refused unless it is owner-only, and it still refuses unknown keys, so a recipe setting written there fails loudly.
- **Loosening needs a grant, tightening does not.** A recipe can set `require_checked_agent = false` only with `unchecked_agents = true` granted. A recipe can ask for `unrestricted` only with the agent granted it. This is a change: before, a grant alone was enough, and no recipe asked for it. Nothing in use was affected, since the one grant in use was for commands.
- **Schema stays 1.** The new keys are additive, and the recipes written so far still validate unchanged. The cost is that an older binary refuses a recipe that uses the new keys (unknown keys are errors). The schema snapshot in `contract/` is regenerated.
- **`fleetify validate` shows the effective settings**: hosts, policy, CI settings, rate limit, the model per agent, what the recipe asks for, and whether the user's grants cover it. A missing grant comes with the `credentials.toml` lines to add. That is the sharing aid: a recipe copied to another person shows them what their file must grant.
- **Sharing a recipe** is copying its directory or committing it to a repository. Nothing in it refers to the machine, so it works for anyone whose grants cover its `needs`. A `pack` command was not built; it is a follow-up if copying by hand turns out to be too much.
- **Migration on this machine.** The grants from `~/.config/fleetify/config.toml` were appended to `credentials.toml` (mode 600 kept), and the old file was renamed to `config.toml.old`. All nine recipes in `~/.fleetify/recipes/` and the examples validate unchanged: they already declared their needs. The e2e harness (`scripts/e2e/run_e2e.py`) used to write a `config.toml` and read credentials from `~/.fleetify`; it now copies `~/.config/fleetify/credentials.toml` into its work folder and passes `--credentials`. The harness was not run here, because it needs real hosts.
- **Code.** `src/config.rs` is removed. `src/grants.rs` holds the grant types. `HostType` and the host table moved to `src/hosts.rs`. The recipe has the new fields and their defaults, and `Recipe::remote_branch`, `host_table`, `model_for` give the effective values. The pipeline reads grants from `Credentials`.
- **Tests.** `tests/config_layer.rs` runs the binary: a leftover `config.toml` refuses in both places, an ungranted command is refused without a credentials file, a granted one passes the grant check (then stops at `check-agent`, which needs no network), and `validate` prints the settings and the grant lines. Unit tests cover the defaults with no file (`recipe.rs`), the host rules, the policy and agent tables, the `unrestricted` and `unchecked` grant checks (`agents.rs`), and the grant section parsing and the refusal of recipe keys in `credentials.toml` (`credentials.rs`). The Python package writes keyed tables (`[hosts."name"]`, `[agents.<name>]`) and has a test for them against the contract schema.

## Fork CI triggers: closed (2026-10-23)

- **Question.** On the icalendar and VoiceStudio forks, pushes and pull requests started no workflow run, while `workflow_dispatch` did. Task `tasks/fork-ci-triggers.md`, now deleted.
- **Cause, as far as the probes show.** A fork that inherits workflow files from its upstream does not start push or pull request runs until a push changes a file under `.github/workflows/`. After that push, every event starts runs. `workflow_dispatch` works throughout. Two observations fit this:
  - **Fork 1** (`dariusgm/icalendar-trigger-probe`, created 16:35:32Z, a fork of `collective/icalendar`). Pushes at 16:35:54 and 16:38:10 started nothing, and neither did a pull request opened at 16:35:59. The first push that changed a workflow file (16:39:30Z) started runs in four seconds. Later plain and empty pushes started runs, and a pull request on a feature branch started runs after a push to that branch.
  - **Fork 2** (`dariusgm/icalendar-trigger-probe-2`, a second fork created at about 16:45Z, the same upstream). Workflows were registered at 16:53:12Z, about eight minutes after creation, where fork 1 took 24 seconds. Pushes at 16:53:09 and 16:53:13 started nothing for ten minutes. The push that changed `.github/workflows/probe-new.yml` (17:03:13Z) started runs in five seconds. A plain file push at 17:03:41Z started runs in five seconds.
  - **Control: a fresh fork with no inherited workflows** (`octocat/Hello-World`). A push that added a workflow file started a run at once. This does not separate the two causes, because the push touched a workflow file.
- **Not checked.** Whether a fork of a repository without workflows, pushed with a plain file change before any workflow edit, is silent. The original icalendar and VoiceStudio forks no longer exist (HTTP 404), so their settings and history cannot be read. The icalendar trial's CI trim touched workflow files; if that commit reached the fork's `main`, this rule would have predicted runs there, so the rule does not fully explain the trial. Only the two probes above support it.
- **Fleetify consequence.** None needed. `ci.dispatch_workflow` covers the silent window on forks. If a push-triggered run is required on a fork that inherits workflows, the recipe can push a change that touches `.github/workflows/` first. That fix is not built or tested in fleetify.
- **Cleanup.** The probe repositories were deleted after the result was recorded. The earlier note in this section about "no setting suppresses triggers" is superseded: no setting was changed, and no setting was found that differs between the silent and active phases.

## Agent containers: first step of stage 3 (2026-10-28)

Task `tasks/agent-confinement-stage-3.md`. This entry covers the container part of stage 3; the other points of that task are not started.

- **Opt in per recipe.** `image` in `recipe.toml` runs the agent in a container. Unset means the host, as before, so no existing recipe or test changed behaviour.
- **Same path inside.** The workspace and each granted directory are mounted at their host path, with `-w` at the workspace. The argv the adapter builds for the host works unchanged, and the prompt stays one argument. The alternative, a remapped path, meant rewriting the Claude `Edit` rules and the copilot grants.
- **Names, not values.** Variables are passed with `-e NAME`, read by the docker client from its own environment. Values never reach an argument list. Container mode forwards only the agent's own variables. `CLAUDE_CONFIG_DIR` is not forwarded, since it names a host path.
- **Model login, one file.** Claude Code's `.credentials.json` is mounted read-only when it exists, and `/root/.claude` is a fresh tmpfs. The agent has to read that file to call its model, so it is in the container's reach. A broker that holds the login outside the container would remove this, and is not built. Copilot and opencode get no login file yet. Copilot gets no variables, as on the host. Opencode gets `ANTHROPIC_API_KEY` only.
- **Image label.** `io.fleetify.agent` must match the agent, checked with `docker image inspect` before a run. `check-agent --image` and the run use the same check.
- **Canary per image.** The `check-agent` marker is keyed by agent, model and image. A host pass does not count for a container run.
- **Stopping.** The docker client dies with a killed run, but the container keeps going. `RunRequest::on_stop` runs `docker rm -f <name>` after fleetify stops the run (timeout, interrupt, waiting for input). A normal exit needs nothing, because `--rm` removes the container. A test checks that a timed-out run leaves no container.
- **Container flags.** `--rm`, `--init` (children are reaped and the tree is signalled), `--cap-drop ALL`, `no-new-privileges`. The container runs as root, which rootless docker maps to the user, so files the agent writes stay owned by the user. Rootful docker would give root-owned files. That is not supported here.
- **Pinned agent versions.** Claude Code 2.1.312 (npm `@anthropic-ai/claude-code`, the install method in its setup docs at code.claude.com), opencode 1.19.3 (npm `opencode-ai`), Copilot CLI 1.0.101 (npm `@github/copilot`, the install page at docs.github.com, 2026-10-28). The versions match the ones on the build host, and each image reports its version (checked 2026-10-28).
- **Orchestrator image.** Multistage: `rust:1-bookworm` builds the binary with `--locked`, and `debian:bookworm-slim` runs it with git and the docker client from `docker:cli`. The orchestrator needs the daemon socket to start agent containers. That is a trust decision the user makes when running it, and the README states it.
- **Not done.** Network restriction per agent (the agent keeps the default network), stricter file-system profiles, event-stream adapters, and a credential-free agent (the model login stays). The stage-3 task keeps those open.
- **Platform.** Container mode is Linux-only. Windows paths are not valid mount sources in a Linux container, and no Windows docker host was tried.
- **Tests.** Unit tests cover the argv (same-path mounts, names not values, prompt last, granted directories once), the name and path checks, the label check against a fake docker, the invocation wrapping, the marker key and recipe validation. `tests/container.rs` runs against the real daemon and the built image, and is ignored by default (`cargo test --test container -- --ignored`). It checks that a host run reads a decoy file outside the workspace and the container does not, that the workspace is writable at its host path, that the agent CLI runs, that a stopped run leaves no container, and that the label check refuses another agent's image.
- **Live canary: passed for Claude Code (2026-10-28).** After the host login was renewed, `fleetify check-agent claude-code --image fleetify-agent-claude-code` passed. The first attempt failed on the expired login ("OAuth session expired" on the host, "OAuth token revoked" in the container), not on the container path. The container got the login through the read-only credentials file only. `CLAUDE_CODE_OAUTH_TOKEN`, which is set in this shell, is not forwarded. Copilot and opencode are not checked yet.

## Recipe-passed variables and agent config (2026-10-28)

- **The recipe names variables, the environment holds the values.** `agent_env = ["NAME", ...]` lists the variables passed into the agent's container. The recipe holds names only. Values come from the orchestrator's environment when the run starts. A run refuses before it starts if a listed variable is not set. Its value is added to the redactor, so logs and reports do not show it. Names must match `[A-Za-z_][A-Za-z0-9_]*` and may not be fleetify's or docker's own variables (`PATH`, `HOME`, `DOCKER_HOST` and similar). Needs `image`.
- **Opencode's provider lives in the recipe.** `[agents.opencode.config]` is a table, written as TOML and sent as JSON. fleetify merges it into `OPENCODE_CONFIG_CONTENT` and writes its own `permission` key over the top, so a recipe cannot widen the grants. A recipe may not set `permission`. Secrets go in as `"{env:NAME}"`, which opencode fills from the agent's environment (checked in the opencode config docs, 2026-10-28), so the key itself is never in the recipe. Only opencode takes a config table.
- **The placed form per agent.** Claude Code: its variables, or its login file (see above). Copilot: its token variables are not forwarded yet, so `agent_env` is the way to give it one. Opencode: the config table, plus any variables its `{env:}` placeholders name.
- **Not solved here.** The agent can still read every variable and file it receives, so this limits what it gets, not what it can do with it. A broker that keeps the key outside the container is still the end state. `check-agent` does not read the recipe, so an opencode canary runs without the recipe's provider and will fail when the provider is needed for an answer. The check of a recipe's provider is the run itself.
- **Tests.** Unit tests cover the name rules, the recipe rules (needs an image, no reserved names, config only for opencode, no `permission` key), and the merge: the config is in the client environment and not on the command line, and the named variables are forwarded. `tests/container.rs::opencode_in_its_container_answers_from_the_recipe_provider` runs the real path (recipe, invocation, container) against a llama.cpp server. It is ignored by default, needs `FLEETIFY_TEST_LLAMACPP_URL`, and passed against the local Vulkan server on port 9931 with the placeholder key supplied through `agent_env`.
