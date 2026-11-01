# Network: proxy denials in the report

**Status:** open, decision needed first.

**Conflict.** `DECISIONS.md` ("Stage-3 network design", proxy denials) says a denied request "becomes an event and a line in the report, with the host and port". The built behaviour, and the note in the same decision ("Not built"), is: denials are `NetworkDenied` events and lines in the run log, not in `report.json`. `tasks/agent-confinement-stage-3.md` says the same.

**Open.**

1. Decide: put denials into `report.json` (a list per repository with target and count), or correct the decision to say events and log only. The working agreement favours the report, because the report is what a person reads first.
2. If the report gets them: add the field to `src/report.rs`, the JSON schema in `contract/`, and a test in `tests/report.rs`. Keep the target and count only, no request bodies.
3. Live streaming of denials is a separate item: they are read when the run ends, so a long run shows nothing until the end.

**Done when.** The decision is written in `DECISIONS.md`, and if the report gets denials, a test asserts a refused request appears in `report.json`.
