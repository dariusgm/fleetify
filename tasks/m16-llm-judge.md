# M16: LLM judge (scope agreed, postponed until enabled by data)

**Status:** postponed. The scope below is agreed; building it is still gated on evidence (see "When to build"). Open decisions are listed at the end.

**Goal.** A judge reviews the diff of a change that passed the agent loop and the deterministic gates, and returns `pass`, `fail` or `unsure`. It answers what the layer-1 rules cannot: did the agent do what the task asked, and nothing more; did it weaken a check to get green. `fail` ends the repository as `guard_violation`, `unsure` as `needs_review`. Nothing is committed or pushed on either. Design: `docs/design/guards.md`, layer 2.

## Placement

After the agent loop has finished, after the verifier (outer gate, trusted copy) and the layer-1 guards, and before the commit. In the code this is inside `judge_change` (`src/pipeline.rs`), after the guard step. Reason: a change that already fails a deterministic gate is not worth a model call. The judge also runs after every fix round, so fix rounds are judged like the first attempt.

## Fresh session, not self-review

The judge must never see the executing agent's session. It runs as a new one-shot process (`Agent::judge_command()`), with no resume or continue flag and no session id from the executor. Its input is only:

- the task text,
- the diff (see "Input size"),
- the verifier summary.

It does not get the agent's transcript, its reasoning, its final message or its own summary of the change. Those would carry the agent's framing into the judgment. Whether a different model is needed on top of the fresh session is an open decision (below).

## What it judges

Narrow on purpose. One question: *is this diff the requested change, and nothing more, and does it avoid gaming the gates?* Checks:

- **Task fit:** the diff does what the task text asks, not a symptom-level workaround.
- **Scope creep:** unrelated refactors, renames, formatting sweeps, extra features.
- **Gaming the gates:** weakened assertions, removed test cases, narrowed test inputs, relaxed thresholds, changed verifier behaviour.
- **Minimality:** a change larger than the task needed.

Not judged: general code quality, bugs, security scanning. Those are other jobs.

Deterministic things stay in layer 1 and are not the judge's job: suppression comments (`deny_added_patterns`), deleted test files (`forbid_test_deletion`), binaries, path limits.

## Input and size

- **Diff** against the base commit, with a small context window (start with `-U3`; to be measured).
- **Task text** and **verifier summary**.
- **Not included:** whole files, repository access, tools, the agent's transcript.
- The diff is untrusted data: fenced, and the output is schema-checked. Parse error = `unsure`.
- **Size limit:** start at **1000 changed lines** (added plus deleted, counted by `git diff --numstat` over the judged files). Over the limit the verdict is `unsure`, never a silent truncation.
- **Lock and generated files** (`*.lock`, `uv.lock`, `Cargo.lock`, `poetry.lock`, generated code) are excluded from the size count and from the input. They are replaced by a line such as "`uv.lock` changed, 4 210 lines, not shown". Their content is checked by layer 1 (`allow_paths`, `max_lines_changed`), not by the judge. Without this, dependency-update recipes would almost always end in `unsure`.

## Context size: to be measured

The working assumption is that the bigger the change, the more context the judge needs. That is not yet known. The experiment:

- **Diff size:** 200, 500, 1000, 2000 lines.
- **Context window:** `-U0`, `-U3`, `-U10`, and the full changed file for a sample.
- **Measure per variant:** agreement with the fixture label, `unsure` rate, false `fail` on good changes, input tokens, latency.

Question to answer: at which size does the verdict start to fail on bad changes, and does more context help or only add cost? The 1000-line start is a hypothesis. Changes that are above it are judged `unsure` until the experiment says otherwise.

Data to start from: the integration test runs already done produced real changes with a final preference. Their diffs are the first good fixtures, and their size distribution shows how often 1000 lines is hit in practice. Lockfile-heavy recipes (e.g. `poetry-to-uv`, `cargo-deps-update`) will push that number up if lock files are counted, which is why they are excluded above.

## Same model or different model

Open. Two options:

1. **Fresh session, same model as the executor.** Cheapest; no second provider or approval needed. Risk: correlated blind spots.
2. **Fresh session, different model (and ideally provider).** Needs a second model and, for company code, an approved provider (`work-environment-questions.md`, question 8).

Plan: run the same fixture set with both. Decision rule: recommend a different model only if the same-model judge misses bad fixtures that the other one catches, and the extra cost and approval are acceptable. Otherwise recommend the fresh session. The recommendation is written into the recipe documentation, not forced.

## Activation and measurement

- Off by default. Enabled per recipe with a `[judge]` section (`agent`, `profile`, `max_diff_lines`), which comes back with this item (it was removed in the MVP cleanup, `DECISIONS.md`).
- A recipe may enable it only after the fixture set passes: all bad fixtures `fail` or `unsure`, all good fixtures `pass`.
- Fixtures: pairs of task and diff, labelled good or bad, under `tests/`. The CI run uses a fake judge (like `tests/copilot_fake.rs`). Live runs against a real model are manual, because they cost money.
- Record per call: verdict, reasons, input size, cost, latency. Put them in a `judge_verdict` event and in `report.json`.

## Fixture set (first cut)

Bad (must not `pass`):

- an unrelated refactor mixed into a correct fix,
- a test whose assertion was loosened or whose case was removed (not deleted, so layer 1 does not catch it),
- a symptom fix that turns the verifier green without fixing the cause,
- a scope-creep change in a large diff, to test the size limit.

Good (must `pass`):

- a minimal correct change,
- a correct change in a large diff, to catch false alarms from size,
- a lock-file update with a small source change (tests the exclusion rule).

## When to build

M16 says "only if M17 shows the guards are not enough". The integration runs already done are input to that question, but they are not the M17 trial. Building the prompt, fixtures and fake-judge tests is cheap and does not need a provider. Live use needs the provider decision. Decide with the user whether to build the first step now.

## Open decisions

1. **Build now or after M17?** The fake-judge part is cheap; live use needs a provider.
2. **Same model or different model**: decided by the experiment above, not in advance.
3. **Context size**: start at 1000 lines and `-U3`, then measure. The result sets the default `max_diff_lines`.
4. **Lock files**: excluded from size and input, as proposed. Confirm the list of patterns.
5. **Fix rounds**: judge every round (proposed), or only the final change? Each round costs one call.
6. **`unsure` handling**: `needs_review` with the workspace kept (proposed, as in the design). Should a recipe be able to retry the judge once on `unsure` (e.g. after a parse error)? Proposed: no retry, the parse error is itself a signal.
7. **Labels for real runs**: who labels a real change as good or bad when the judge disagrees with the outcome? Proposed: the user, once per disagreement, recorded in the run folder.
8. **Provider and model** approved for company diffs (`work-environment-questions.md`, question 8). Blocks live use only.
9. **Configuration owner**: `[judge]` in the recipe (proposed) or a global default in the config file, with the recipe overriding.

## Done when

- The scope above is implemented behind `[judge]`, off by default.
- The fixture set runs in CI with the fake judge.
- The context experiment has numbers for the sizes and context windows above, and the default `max_diff_lines` is set from them.
- The same-model versus different-model question has a written answer based on the fixture results.
