# Data contract: recipe.toml

`recipe.schema.json` is the JSON Schema of recipe.toml, as `fleetify schema` prints it. It is the
contract between the two sides:

- **Rust (`cargo test`, `tests/contract.rs`)** checks that this file equals the binary's own schema,
  and that the golden recipe `examples/recipes/enable-gitlab-ci/recipe.toml` passes `fleetify validate`.
- **Python (`authoring/`, job `authoring` in CI)** writes the same recipe and checks that its output
  equals the golden file byte for byte and passes this schema. The Python job does not build the binary.

When the schema changes on purpose, regenerate the file and commit it with the change:

    cargo build && ./target/debug/fleetify schema > contract/recipe.schema.json
