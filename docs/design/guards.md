# Guards (diff guards)

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

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

