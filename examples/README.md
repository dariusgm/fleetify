# Example recipes

Each directory here is one recipe: a `recipe.toml` with its prompts, tools and attachments. A recipe is portable. Nothing in it refers to a machine, so a copy works for anyone whose grants cover what it asks for.

What a recipe asks for is in its `[needs]` table (commands, web access, directories, `unrestricted`) and its `[policy]` table. The grants that allow it are in the user's `~/.config/fleetify/credentials.toml`. Check a recipe on your machine with:

    fleetify validate examples/recipes/cargo-deps-update

The last lines show the effective settings, and, for anything not yet granted, the `credentials.toml` lines to add. For example, `cargo-deps-update` asks for `cargo`:

    [permissions]
    commands = ["cargo"]

`enable-gitlab-ci` is the golden recipe: `authoring/examples/enable_gitlab_ci.py` writes it byte for byte, and the tests check that.
