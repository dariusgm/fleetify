# Claude Code

**Recipe value:** `agent = "claude-code"`, `model = "haiku"`.

## What we tested (2026-11-06)
- Canary: passes with `--model haiku`. Haiku is the alias the Claude CLI accepts.
- E2E, inner loop on, Jev judge, base correct: readme pushed (Jev pass, confidence 1.00); deps pushed (Jev pass 0.92); gaming: the agent made the honest fix, Jev answered fail at 0.53, so the run ended as `guard_violation`.
- The inner loop works: the agent ran `sh .agent/tools/verify.sh .` itself, and nothing was denied.
- Earlier, before the base and inner-loop fixes, deps was blocked because the agent could not look up crate versions. Its web access is not granted by default.

## Pros
- Predictable on the small cases so far: readme and deps both ended as expected.
- The permission model is precise. The verifier is granted as the full command, `Bash(sh .agent/tools/verify.sh .:*)`, not the whole interpreter.
- Haiku is cheap, and `--model haiku` is a stable alias, so a run is reproducible.

## Cons
- It stops with `needs_input` when it cannot run a tool, and then writes `blocked.md`. A recipe has to say what to do in that case.
- Dependency bumps need current crate versions. The grant for this is `cargo`, which Claude Code matches as `Bash(cargo:*)`: every cargo subcommand is allowed, not only lookups. That is broader than the lookup needs (see the decision below).
- The gaming case is judged by Jev on a diff that is a correct fix; Jev's answer on that diff was unstable before the prompt change (see `tasks/m16-jev-prompt`).

## Decision context
Pick Claude Code when you want a pinned model and an exact permission grant. Its cost is the Anthropic account, which is separate from Copilot. Check the claude-code recipe's model before a comparison, because the CLI's default model changes.

## Decision (2026-11-07)
The `cargo` grant for lookups is accepted as it is, `Bash(cargo:*)`. A narrower rule such as `Bash(cargo search:*)` and `Bash(cargo info:*)` is possible but not set; add it if a recipe should not be able to run `cargo publish` or other subcommands.

## Open
- Nothing blocks Claude Code now. Keep the e2e recipes on `min_confidence = 0.5`.
