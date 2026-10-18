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
