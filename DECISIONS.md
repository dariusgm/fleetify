# Decisions

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
- **Plain renderer** lives in `ui.rs` (M1 scope); the live view is M13.

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
- **M8 waves: not started.** `--waves` and `--continue-if-green` are refused.
- **M9 agent runner: partly done.** Present: task assembly from instructions and
  attachments, allowlisted environment, timeout, `blocked.md`, `needs` checked
  against grants, agent-made commits normalised to the base. Missing: the fixed
  rules block and verifier command in `task.md`, the `.agent/tools` copy, the
  inner-loop verifier, `result.md`, `wants_input()` and the idle event, the
  `permissions_effective` event and the run-header warnings, and the output-line
  cap noted above.
- **M10 opencode and check-agent: partly done.** `check-agent` exists for
  claude-code only. Its canary is simpler than the plan: there is no decoy marker
  and no cache per agent version. The opencode adapter is not started.
- **M11 CI gate and merge: not started.** `src/ci.rs` is a stub.
- **M12 Python authoring package: not started.**
- **M13 live view: not started.** `--ui` and `--plain` are parsed but have no effect.
- **M14 Claude Code adapter: done in code, unverified.** The CLI is not installed
  on the development machine, so no `[ASK]` test has run.
- **M15 Copilot CLI adapter: not started.**
- **M16 LLM judge: not started.** `judge` is accepted by the parser and not used.
- **M17 real trial: not started.**

Declared but unused: `ci.wait`, `judge`, `limits.idle_warn_min`, `canary_repos`.

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
