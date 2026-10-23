# Non-goals

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

- Other hosts than GitHub and GitLab, other agents than Claude Code, opencode and Copilot CLI. Anyone who needs them implements the `Host` or `Agent` trait.
- CI systems other than GitLab CI in stage 1. A `CiProvider` is one module and can come later or from the community.
- Existing local checkouts.
- Logic in the recipe format. Logic lives in `tools/` scripts.
- Running Python at runtime. The Python package only generates files.
- Interactive multi-terminal views (tmux style), log browsing in the TUI.
- Configuring an agent's provider, model or login. (The permissions in the invocation are fleetify's job and in scope.)

