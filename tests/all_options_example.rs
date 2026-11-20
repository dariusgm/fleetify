//! The `all-options` example: it validates, and it names every option the recipe schema has, so
//! a new option that is not shown in the example fails this test (see its README.md).

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;

use fleetify::recipe::Recipe;

fn example_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/recipes/all-options")
}

#[test]
fn the_all_options_example_validates() {
    let output = Command::new(env!("CARGO_BIN_EXE_fleetify"))
        .arg("validate")
        .arg(example_dir())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("recipe all-options is valid"), "{stdout}");
}

/// Every property of every table in the schema, by name.
fn collect_properties(schema: &serde_json::Value, names: &mut BTreeSet<String>) {
    match schema {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Object(properties)) = map.get("properties") {
                names.extend(properties.keys().cloned());
            }
            for value in map.values() {
                collect_properties(value, names);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_properties(item, names);
            }
        }
        _ => {}
    }
}

#[test]
fn the_all_options_example_names_every_option_of_the_schema() {
    let text = fs::read_to_string(example_dir().join("recipe.toml")).unwrap();
    // The recipe parses with every table the schema knows.
    Recipe::parse(&text).unwrap();

    let schema: serde_json::Value = serde_json::from_str(&fleetify::recipe::json_schema()).unwrap();
    let mut names = BTreeSet::new();
    collect_properties(&schema, &mut names);
    assert!(
        names.len() > 60,
        "the schema walk found only {}",
        names.len()
    );

    // An option is shown when its line appears, active or commented out: `name = ...`, or a
    // table header `[name]`, `[name.sub]` or `[[name]]`.
    let missing: Vec<&String> = names
        .iter()
        .filter(|name| {
            let name = regex::escape(name);
            let shown =
                regex::Regex::new(&format!(r"(?m)^\s*#?\s*(\[{{1,2}}{name}[\].]|{name}\s*=)"))
                    .unwrap();
            !shown.is_match(&text)
        })
        .collect();
    assert!(
        missing.is_empty(),
        "options missing from examples/recipes/all-options/recipe.toml: {missing:?}"
    );
}
