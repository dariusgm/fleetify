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
- **Edition 2021, MSRV 1.75** (conservative; the toolchain here is newer).

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
