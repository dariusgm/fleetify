//! Environment variables for fleetify and its agents, read from the process environment and from
//! a `.env` file in the folder fleetify runs from. A variable already set in the process
//! environment wins over the file. Every read of a name fleetify forwards or needs goes through
//! [`var`] or [`var_os`], so the file counts the same as the shell.
//!
//! The file is `KEY=VALUE` per line. `#` starts a comment line, a leading `export ` is allowed,
//! and a value may be wrapped in single or double quotes. Values are never logged; see
//! [`file_values`] for the redactor.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::sync::OnceLock;

/// The file fleetify reads from its working directory.
pub const FILE_NAME: &str = ".env";

static FROM_FILE: OnceLock<BTreeMap<String, String>> = OnceLock::new();

/// Loads `.env` from `dir`, once, at startup. A missing file is not an error. Returns the
/// number of variables read, or an error that names the line but never the value.
pub fn load_from(dir: &Path) -> Result<usize, String> {
    let path = dir.join(FILE_NAME);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("cannot read {}: {err}", path.display())),
    };
    let values =
        parse(&text).map_err(|line| format!("{}: line {line} is not KEY=VALUE", path.display()))?;
    let count = values.len();
    // A second call keeps the first load: the file is read once per process.
    let _ = FROM_FILE.set(values);
    Ok(count)
}

/// Reads `KEY=VALUE` lines. Returns the 1-based number of the first bad line.
pub fn parse(text: &str) -> Result<BTreeMap<String, String>, usize> {
    let mut values = BTreeMap::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line
            .strip_prefix("export ")
            .map(str::trim_start)
            .unwrap_or(line);
        let Some((name, value)) = line.split_once('=') else {
            return Err(index + 1);
        };
        let name = name.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(index + 1);
        }
        values.insert(name.to_string(), unquote(value.trim()).to_string());
    }
    Ok(values)
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return &value[1..value.len() - 1];
        }
    }
    value
}

/// The value of `name`: the process environment first, then the file.
pub fn var(name: &str) -> Option<String> {
    var_os(name).and_then(|value| value.into_string().ok())
}

/// As [`var`], keeping non-UTF-8 values.
pub fn var_os(name: &str) -> Option<OsString> {
    std::env::var_os(name).or_else(|| {
        FROM_FILE
            .get()
            .and_then(|values| values.get(name))
            .map(OsString::from)
    })
}

/// The values read from the file, for the redactor. The process environment is not included.
pub fn file_values() -> Vec<String> {
    FROM_FILE
        .get()
        .map(|values| values.values().filter(|v| !v.is_empty()).cloned().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_plain_quoted_and_exported_lines_and_skips_comments() {
        let text = "# a comment\n\nTYPESAFE_API_KEY=abc123\nexport OPENAI_API_KEY=\"sk-x y\"\nNAME = 'single'\n";
        let values = parse(text).unwrap();
        assert_eq!(values["TYPESAFE_API_KEY"], "abc123");
        assert_eq!(values["OPENAI_API_KEY"], "sk-x y");
        assert_eq!(values["NAME"], "single");
        assert_eq!(values.len(), 3);
    }

    #[test]
    fn a_bad_line_is_reported_by_number_without_its_value() {
        let err = parse("GOOD=1\nthis line has a secret value\n").unwrap_err();
        assert_eq!(err, 2);
        let err = parse("BAD NAME=1\n").unwrap_err();
        assert_eq!(err, 1);
    }

    #[test]
    fn a_missing_file_gives_no_values() {
        let dir = std::env::temp_dir().join(format!("fleetify-envfile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load_from(&dir).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
