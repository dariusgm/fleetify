# Guards (diff guards)

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

Guards decide whether the agent's diff may leave the machine. They are layered so that the cheap, reliable layer always runs and the expensive, fuzzy layer is optional.

**Layer 1: deterministic rules (always on, no LLM).** Evaluated on the diff against the base commit (computed with line-ending differences ignored). Fields: `allow_paths` (optional whitelist; anything else fails), `deny_paths`, `deny_added_patterns`, `max_files_changed`, `max_lines_changed`, `forbid_test_deletion` (deleting or renaming-away a test file fails; file level, by path pattern; weakened test cases are the judge's job), `forbid_binary_files`. `.agent/**` is always denied regardless of the recipe. A violation ends the repo as `guard_violation` with the rule and the offending lines in the report. No push, workspace kept.

**Layer 2: LLM judge (optional, off by default).** Answers questions rules cannot: is this diff what the task asked for, or did the agent refactor unrelated code, weaken a check or "fix" the symptom? Design and run record: `tasks/m16-llm-judge.md`.

- **Placement:** after the agent, the verifier (trusted copy) and the layer-1 guards pass, immediately before the commit and push. Nothing is pushed and no CI is triggered before the verdict. A fix round (CI failure) passes through the judge again before its push.
- **What it is:** a separate, fresh, one-shot LLM call. Not the executing agent's session, no tools, no repo access. Input: the task text, the verifier summary, a deterministic summary of changed lock files, and the diff with `-U3` by default (`diff_context` changes it; lock files excluded). Output: `{"verdict": "pass" | "fail" | "unsure", "reasons": [...]}`. A missing or unparseable answer counts as `unsure`.
- **Configuration: explicit per recipe, no default model.** The section is absent by default, and a recipe without `[judge]` runs no judge. Every value is in the recipe; nothing is inherited from a config file or the environment:

  ```toml
  [judge]
  level = "session"                  # session | model | provider
  max_diff_lines = 10000             # over this: verdict unsure, no model call
  context_tokens = 32768             # the judge model's context window; required
  output_tokens = 8192               # reserved for the answer; required, below context_tokens
  # timeout_s = 1800                 # one call's limit in seconds (1 to 7200); default 1800
  # diff_context = 3                 # unchanged lines around each change in the diff (0 to 100); default 3
  # agent = "opencode"               # required for model and provider; rejected for session
  # model = "llamacpp/qwen3.8-flash-next"   # required for model and provider; rejected for session
  # task = "..."                     # the task text; unset: the recipe's instructions
  ```

  - The input must fit the window after the answer and about 8000 tokens of call overhead; otherwise the verdict is `unsure` without a call. The stricter of `max_diff_lines` and this token budget applies.
  - `session`: a fresh session with the executor's agent and model. The minimum level. It is the only level for a recipe whose executor is the local model and the judge is the same model.
  - `model`: another model, from the same provider or another one. `model` must differ from the executor's model.
  - `provider`: a model from a different provider than the executor. A scripted recipe (no agent) has no executor provider to match, so `provider` is accepted there.
  - Refused at validation: `session` without an executor; `session` with its own `agent` or `model`; `model` or `provider` without an explicit executor model when an executor exists (an agent default cannot be compared); the judge agent other than opencode, claude-code or copilot.
- **Fresh call, enforced:** the judge argv comes only from `agents::judge_command()`: no resume, continue or session flag. Its environment is the base allowlist plus the judge's own configuration, so no executor variable reaches it. It runs in an empty directory under the temp dir, which is a git repository holding only the input file, and that directory is removed afterwards. The judge's config allows reading and nothing else; a read outside its directory is refused by opencode's `external_directory` rule (checked by hand).
- **One-directional:** the judge can only veto. `fail` ends the repo as `guard_violation` with the rule `judge`; `unsure` ends it as `needs_review`. Nothing is pushed on either, and the workspace is kept. A `pass` never overrides layer 1 or the verifier.
- **Untrusted input:** the diff is written by the agent and may contain instructions aimed at the judge. It is passed as fenced data (a fence longer than any backtick run in the diff), the judge has no tools, and its output is schema-checked.
- **Big diffs:** over `max_diff_lines` the verdict is `unsure`, never a silent truncation. Lock files are not counted and are summarised per package (added, removed, changed).
- **Record:** every call writes `judge.json` (level, agent, model, sizes, lock summary, verdict, reasons, duration) and `judge-input.md` into the repo folder of the run.
- **Evaluation:** a fixture set of good and bad diffs. The judge is only enabled for a recipe after it catches the bad ones on that set. Cost and latency are recorded per call.

## Verification results

The grid of implementation tool x judge tool, the stability runs and the positive changes are in `docs/design/judge-verification.md`.
