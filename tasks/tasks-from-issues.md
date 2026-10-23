# Idea: tasks from issues (fan-out)

**Status:** idea only, not scheduled. The owner wants to think it over before any work.

**Idea.** A GitHub issue or a GitLab work item describes one change and lists the repositories it applies to. fleetify reads the issue, turns the target list into a repository list, and runs the recipe over those repositories. Pull requests link back to the issue, and the result per repository is posted as a comment.

**Points to settle first.**

- **The target list is untrusted input.** It decides which repositories get write attempts. The same rules as for `repos.txt` apply: only configured hosts, no invented URLs.
- **Dry run first.** The fan-out shows which repositories it would touch, and needs an explicit go before a real run.
- **Hard limit.** A maximum number of repositories per issue.
- **Author or label filter.** Only issues from allowed authors, or with a fixed label, are processed.
- **Report back.** One comment per issue with the result per repository.
- **Jira** would be a second source adapter, only if the company works with Jira.

**Done when.** The owner has decided on the points above, and the design is written into docs/design/.
