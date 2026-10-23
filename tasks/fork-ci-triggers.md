# Open question: no workflow runs on a fork for push or pull request

**Status:** open, cause not known. Observed on the public forks of the icalendar and VoiceStudio trials. The GitLab trial runs did not use forks.

**What is known.**

- Pushes to `main` and to feature branches, and pull requests inside the fork, start no workflow run.
- `workflow_dispatch` on the same branch starts a run at once.
- Actions are enabled, the workflow files are active.
- The fork's events show the push and the pull request.

**Hypotheses.** A setting on the fork that suppresses automatic triggers; a rule for workflows of forks created from a public repository; the `on:` filter in the workflow file. Only the first one is worth a test.

**Next steps.**

1. Create a new throwaway private repository with a one-line workflow that has `on: push`. Push to it. Note whether a run starts.
2. If not, look at the repository's Actions settings and the workflow permissions.
3. Write the result in DECISIONS.md. If the cause is a setting, document how to set it, and delete the throwaway repository.

**Done when.** The cause is known, or the trial documentation says fleetify must dispatch CI on forks (which it does now) with the cause marked as unknown.
