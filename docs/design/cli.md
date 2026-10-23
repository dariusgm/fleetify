# CLI

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

```
fleetify run <recipe-dir> repos.txt [--dry-run] [--only a,b] [--parallel 3] [--run NAME]
            [--agent NAME] [--waves 1,all] [--canary-repos a,b] [--continue-if-green]
            [--merge] [--allow-unchecked-agent] [--plain | --ui] [--tail N]
            [--format table|json] [--report PATH] [--debug]
fleetify validate <recipe-dir>     # schema check, resolve files, print rendered commands per OS
fleetify schema                    # JSON Schema of recipe.toml for this binary
fleetify report [--run NAME] [--format table|json] [--failed]
fleetify status [--run NAME] [--watch]
fleetify logs <repo> [--run NAME] [-f] [-n 50]
fleetify check-agent NAME          # canary test: is the agent confined to its workspace?
fleetify clean [--run NAME]        # delete workspaces of a run (clones stay)
```

`--dry-run` clones or fetches the cache, evaluates `applies_to`, shows per repo what would happen (including the idempotency verdict), which credential rule matches, the effective permissions per agent, and checks read access at the host. It creates no workspace, starts no agent and pushes nothing. It produces a report like a real run, with result `dry_run` per repo.

