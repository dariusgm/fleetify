# fleetify-recipes

An optional authoring aid. It writes `recipe.toml` files for `fleetify`. The `fleetify` binary never runs Python.

```python
from fleetify_recipes import AppliesTo, Ci, Limits, Recipe, write

write(
    "recipe.toml",
    Recipe(
        name="example",
        branch="chore/example",
        pr_title="chore: example",
        applies_to=AppliesTo(host_type=["gitlab"]),
        ci=Ci(provider="auto", wait=False, timeout_min=30, merge="off"),
        limits=Limits(agent_timeout_min=60, idle_warn_min=10),
    ),
)
```

- Fields that stay `None` (or an empty list) are left out of the file.
- `hosts` (`[hosts."<name>"]`) and `agents` (`[agents.<name>]`) are dicts of dataclasses; they are written as keyed tables.
- `policy` and `rate_limit` mirror the `[policy]` and `[rate_limit]` tables. Grants are not part of a recipe: they live in the user's `credentials.toml`.
- Keys are written in declaration order. One blank line precedes each table.
- A string with a backslash is written as a TOML literal string (single quotes). `Literal` forces that for any string, which is needed to reproduce hand-written files that quote a plain value that way.
- `write()` refuses a recipe whose `schema` is not 1.
- `problems(text, fleetify_schema())` checks a recipe against the JSON Schema of the `fleetify` binary that will run it. It needs the `validate` extra (`jsonschema`).

`examples/enable_gitlab_ci.py` reproduces `examples/recipes/enable-gitlab-ci/recipe.toml` byte for byte. The tests check that, `fleetify validate` on the generated file, and the schema check. They use the debug build at `target/debug/fleetify` (or `$FLEETIFY_BIN`) and skip when no binary exists.

Run: `uv run --group dev pytest` in this directory.
