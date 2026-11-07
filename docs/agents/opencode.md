# opencode

**Recipe value:** `agent = "opencode"`, `model = "llamacpp/qwen3.8-flash-next"`. This runs against a local llama.cpp server (see the README, "Example: opencode against a local llama.cpp server").

## What we tested
- Canary: passes with the local model.
- Earlier (2026-11-04/05), before the inner-loop and base fixes: readme and deps pushed; the honest fix on the gaming case pushed; the scripted judge agreed with every verdict.
- E2E on 2026-11-06 (Jev judge): readme pushed (Jev pass, 1.00); deps pushed (Jev pass, 0.60); the gaming case reached the agent step on the wrong base (`main`), hit its 30-minute limit, then its 60-minute limit on the correct base. It was stopped without a result.
- Not yet re-run with the inner loop and the verifier grant. Its result on the correct base is open.

## Pros
- No external model and no account: the local server runs on your hardware.
- Permissions are a complete deny-by-default configuration (`opencode_config`), so the agent gets only the grants it is given.
- The model can be changed by editing the local server, without changing fleetify.

## Cons
- **Slow on the hard case.** On the gaming case it read source files for most of an hour. Part of that was the wrong base, where nothing was broken; the correct base is still untested.
- **Depends on the local server.** If it is down or busy, the canary and the runs fail.
- **Model quality is the agent's quality.** The local quantised model is not the Claude or Copilot model, so comparisons are about the setup as much as the agent.

## Decision context
Pick opencode when you cannot send code or prompts to an external model, and you have the hardware and time. Expect the hard case to be slow; use the readme and deps cases to check a setup before a gaming-style case.

## Open
- Run the gaming case on `m16/base-broken` with the inner loop and the verifier grant. The 60-minute limit is the first thing to check.
