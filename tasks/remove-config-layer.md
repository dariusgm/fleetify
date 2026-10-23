# Remove the config.toml layer

**Status:** done on branch `feature/remove-config-layer` (2026-10-23), merge request open. The choices are in DECISIONS.md, "Remove the config.toml layer".

**Why.** Behaviour settings live in two places today: the recipe and `config.toml`. A recipe does not show the settings that come from the config, so it is not reproducible on another machine. The decision is that every behaviour setting lives in the recipe, and built-in defaults apply where the recipe is silent.

**Scope.** `credentials.toml` stays the only separate file. These move into the recipe:

- `[ci]` in the config: `dispatch_after_s`, `log_lines`, `log_chars`.
- `[agents.<name>] model`.
- `rate_limit.max_wait_min`.
- `policy` (for example `require_checked_agent`).
- `permissions` (commands, web, directories, unrestricted).
- `merge_policy`.
- `branch_prefix`.
- The hosts list (`[hosts."..."] type`).

**Open questions.**

1. Where the hosts list goes. Options: a `hosts` key in the recipe, or derived from the repository line (`github:`, `gitlab:` and a full URL for self-hosted hosts). A recipe must still say which host type a self-hosted URL belongs to.
2. How `permissions` reach a recipe without a second file. The grant belongs to the user, not to the recipe, so a recipe can only ask (through `needs`) and fleetify refuses a run that asks for more than what it has. Decide where the user grants live if not in `config.toml`.
3. Whether the built-in defaults are the same as today's config defaults (`require_checked_agent = true`, no commands granted, `rate_limit` 15 minutes).

**Done when.**

- `config.toml` is no longer read. A run with one present refuses with a clear message.
- Every setting in the list above is readable from the recipe, with a default when absent.
- `fleetify validate` prints the effective settings of a recipe.
- The schema snapshot, the Python authoring mirror and the README are updated; DECISIONS.md records the choice for each open question.
- Tests cover: defaults without any file, refusal of a leftover `config.toml`, and the permission check against a recipe's `needs`.
