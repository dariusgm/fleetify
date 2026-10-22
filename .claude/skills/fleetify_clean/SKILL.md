---
name: fleetify_clean
description: Bring the fleetify project and its test footprint to a clean state - everything integrated into main, no open branches, all test repos and forks deleted, all local clones removed. Use when the user asks to clean up after trials, or to reset the fleetify test environment.
---

# fleetify_clean

Goal state:

1. Everything the user wants is integrated into `main` of `dariusgm/fleetify`. No open PRs.
2. Only `main` remains as a remote branch. Local: only `main`, or branches the user explicitly keeps.
3. All fleetify test repositories and all forks created for trials are deleted (GitHub and GitLab).
4. All local clones and run folders created by fleetify or trials are removed.

## Safety rules (apply before every deletion)

- **Look before deleting.** List the target, show the list to the user when it is not already explicitly named, then delete.
- **Only what is clearly a test artefact.** A repository counts as a test repo when its name or description says so (for example `fleetify-e2e-*`, `fleetify-m10-smoke`, or a description containing "throwaway"), or when it is a fork created for a trial. Never delete the user's own repositories, even when they are public or look old.
- **Forks:** `gh repo list <owner> --fork` lists them. Delete only the forks the trials created (names of the trial targets, for example `icalendar`, `VoiceStudio`). Check `isFork` before deleting.
- **GitLab deletes with a delay.** `glab repo delete <full/path> --yes` marks a project for deletion; the removal follows later. Report that instead of claiming the project is gone.
- **Branches:** a remote branch is safe to delete when `git branch -r --merged origin/main` lists it. Squash merges are not detected by that check: compare the branch's changed files with `main` (for example `git diff <branch-tip> <merge-commit>`) before deleting, and say so in the report. Unmerged branches are never deleted without the user's confirmation.
- **Local clones to remove:** `~/fleetify-*` scratch files, `~/trials/`, and fleetify's own cache under `~/.fleetify/` (`clones/`, `runs/`, `tmp/`, `e2e-*`, `fleetify.log`). Keep `~/.fleetify/recipes/` and `~/.fleetify/agents/` unless the user asks for them to go (they are not clones). Keep `~/.config/fleetify/` (config and credentials). Keep the project checkout `~/fleetify` itself.

## Steps

1. Integrate open work: `gh pr list --state open`, check the CI (`gh pr checks <n>`), merge with `gh pr merge <n> --merge` only when every check has completed with SUCCESS. Pull `main` afterwards.
2. Remote branches: list the merged ones, delete them with `git push origin --delete <branch>`. Handle squash-merged branches as described above.
3. Local branches: `git branch --merged main` ones with `git branch -d`; list the rest and ask.
4. GitHub test repos and forks: list, confirm the set with the user, delete with `gh repo delete <owner>/<name> --yes`.
5. GitLab test projects: `glab repo list --mine`, filter by the test names, confirm the set, delete each by its full path.
6. Local clones: remove the directories listed above. Check with `du -sh` before and after.
7. Verify: `gh repo list <owner> --limit 100`, `git branch -r`, `git branch`, `glab repo list --mine`, and report what remains and why.

## Report

Say in German which items were deleted, which were left and why (for example GitLab's delayed deletion, branches the user must decide on), and which steps could not be done.
