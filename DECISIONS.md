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
