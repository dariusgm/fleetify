# Live view (optional, demo)

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

`--ui` (only when stdout is a TTY; otherwise ignored with a note) starts a ratatui renderer on the event stream:

- One screen, no navigation: a table with one row per repo (`repo`, `phase`/result, `elapsed`, `idle`, last output line); active repos first, finished repos sorted below; a footer with the running summary (`5 running, 2 done, 1 failed`) and the wave prompt when a wave ends.
- A repo whose last event is a problem (idle too long, `needs_input`, `blocked`, `verify_failed`, `guard_violation`, CI failed) is highlighted and expands to show the last `--tail N` lines (default 6) of the failing output.
- `q` or Ctrl-C behaves like Ctrl-C in plain mode. Respect `NO_COLOR`. On any terminal error it falls back to the plain renderer without losing events.
- Deliberately not in scope: scrolling through logs, switching views, editing anything. `fleetify logs` and the report cover that.

