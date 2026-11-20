# Example: every recipe option

`recipe.toml` sets every option a recipe can have, once, with a comment on each. The comments
say whether the option is **required** or **optional**, what its default is when left out, and
what it is for. Read the file top to bottom; the sections follow the order of a run: the
repository filter, the agent, prepare, verify, guards, the judge, the permissions it needs,
the network, CI, delivery, limits and policy.

Check it without running anything:

    fleetify validate examples/recipes/all-options

The validator prints the effective settings and, for each permission the recipe asks for that
the user has not granted, the `credentials.toml` lines to add.

## What the example is not

- It is not a change to run as it is. Its `path_glob`, `[hosts]`, `[agents.opencode]` and
  `ignore` values name placeholder hosts and paths. Copy the file and keep only the options you
  need.
- It does not set `[script]` next to `[prepare]`: the two exclude each other. The commented-out
  `[script]` block shows the other form. A complete scripted recipe is `examples/recipes/scripted-update`.
- Some options only make sense together. `image` needs `[network]`, `isolation` and `agent_env`
  need `image`, `delivery.delete_branch` needs `ci.merge = "when_green"`, and the judge levels
  need the matching fields. Each comment names its requirement.

## Keeping it complete

`tests/all_options_example.rs` reads the recipe schema and fails when an option has no line in
`recipe.toml`, active or commented out. A new option therefore needs a line here, with its
comment, before the tests pass.
