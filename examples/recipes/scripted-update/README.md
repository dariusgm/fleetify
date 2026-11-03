# Example: a scripted change without an agent

The change is a script, not a prompt. No LLM runs, so the change is repeatable and cheap:

```toml
[script]
posix = "update.sh"      # run with bash
windows = "update.ps1"   # run with PowerShell 7 (pwsh)
timeout_s = 600
```

- Each file is a name inside `tools/`. A script for only one OS is allowed; on the other OS the run refuses with a message that names the missing script.
- The script gets the repository path as its only argument, like the verifier.
- Set either `[script]` or `[prepare]`, not both.
- The verifier, the guards and the CI decide what happens next, as for an agent's change. A failing script ends the run as `error` at the prepare step, before any push.
- To add an agent later, add `[[instructions]]`: the agent then runs after the script, on the prepared tree.

The example re-resolves a uv lock file. `repos.txt` names a test repository. The PowerShell scripts are checked by the tests for their command line only: pwsh is not installed on the machine that wrote them, so run them once on a machine with PowerShell 7 before the first real run.

Check the recipe without running anything:

    fleetify validate examples/recipes/scripted-update
