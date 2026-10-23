# M16: LLM judge (postponed)

**Status:** postponed. Only if the real trial (M17) shows that the deterministic guards are not enough. The `[judge]` section was removed from the recipe format in the MVP; it comes back with this item.

**Goal.** A judge reviews the diff of a change that passed the gates and returns `pass`, `fail` or `unsure`. `fail` ends the repository as `guard_violation`, `unsure` as `needs_review`. Nothing is pushed on `fail`.

**Acceptance.** It catches the bad fixtures (a suppression comment, a deleted test, an unrelated refactor, a weakened check), returns `unsure` for oversized diffs and malformed output, and records cost and latency per run.

**Open questions.** Which provider and model are approved for company code (see `work-environment-questions.md`).
