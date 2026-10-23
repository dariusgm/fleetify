//! Diff guards, the outer gate before anything is pushed (docs/design/guards.md, "Guards").
//! Pure functions over parsed git output, so every rule can be tested without a
//! repository. A violation names the rule and the offending lines.

use regex::Regex;

use crate::credentials::glob_match;
use crate::recipe::Guards;

/// One file in the staged diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    /// `None` for binary files (git prints `-`).
    pub added: Option<u64>,
    pub deleted: Option<u64>,
    /// Status letter from `--name-status`: A, M, D, R, ...
    pub status: char,
}

/// A rule that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub rule: &'static str,
    pub message: String,
    pub lines: Vec<String>,
}

const MAX_SHOWN_LINES: usize = 5;

/// Joins `git diff --numstat` and `--name-status` output. Paths are matched by name.
pub fn parse_changes(numstat: &str, name_status: &str) -> Vec<FileChange> {
    let statuses: Vec<(char, String)> = name_status
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let status = parts.next()?.chars().next()?;
            let path = parts.next_back()?.to_string();
            Some((status, path))
        })
        .collect();
    numstat
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let added = parts.next()?;
            let deleted = parts.next()?;
            let path = parts.next()?.to_string();
            let status = statuses
                .iter()
                .find(|(_, p)| *p == path)
                .map(|(s, _)| *s)
                .unwrap_or('M');
            Some(FileChange {
                path,
                added: added.parse().ok(),
                deleted: deleted.parse().ok(),
                status,
            })
        })
        .collect()
}

/// The added lines of a unified diff (without the `+++` file headers).
pub fn added_lines(patch: &str) -> Vec<String> {
    patch
        .lines()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .map(|l| l[1..].to_string())
        .collect()
}

/// Checks every guard. Returns all violations, not just the first.
pub fn check(changes: &[FileChange], added: &[String], guards: &Guards) -> Vec<Violation> {
    let mut violations = Vec::new();

    if changes.len() > guards.max_files_changed as usize {
        violations.push(Violation {
            rule: "max_files_changed",
            message: format!(
                "{} files changed, limit is {}",
                changes.len(),
                guards.max_files_changed
            ),
            lines: Vec::new(),
        });
    }

    let total: u64 = changes
        .iter()
        .map(|c| c.added.unwrap_or(0) + c.deleted.unwrap_or(0))
        .sum();
    if total > u64::from(guards.max_lines_changed) {
        violations.push(Violation {
            rule: "max_lines_changed",
            message: format!(
                "{total} lines changed, limit is {}",
                guards.max_lines_changed
            ),
            lines: Vec::new(),
        });
    }

    let binary: Vec<String> = changes
        .iter()
        .filter(|c| c.added.is_none())
        .map(|c| c.path.clone())
        .collect();
    if guards.forbid_binary_files && !binary.is_empty() {
        violations.push(Violation {
            rule: "forbid_binary_files",
            message: "binary files changed".into(),
            lines: binary,
        });
    }

    let deleted_tests: Vec<String> = changes
        .iter()
        .filter(|c| c.status == 'D' && c.path.to_ascii_lowercase().contains("test"))
        .map(|c| c.path.clone())
        .collect();
    if guards.forbid_test_deletion && !deleted_tests.is_empty() {
        violations.push(Violation {
            rule: "forbid_test_deletion",
            message: "test files were deleted".into(),
            lines: deleted_tests,
        });
    }

    let denied: Vec<String> = changes
        .iter()
        .filter(|c| guards.deny_paths.iter().any(|p| glob_match(p, &c.path)))
        .map(|c| c.path.clone())
        .collect();
    if !denied.is_empty() {
        violations.push(Violation {
            rule: "deny_paths",
            message: "changes touch denied paths".into(),
            lines: denied,
        });
    }

    if !guards.allow_paths.is_empty() {
        let outside: Vec<String> = changes
            .iter()
            .filter(|c| !guards.allow_paths.iter().any(|p| glob_match(p, &c.path)))
            .map(|c| c.path.clone())
            .collect();
        if !outside.is_empty() {
            violations.push(Violation {
                rule: "allow_paths",
                message: "changes outside the allowed paths".into(),
                lines: outside,
            });
        }
    }

    for pattern in &guards.deny_added_patterns {
        // The recipe was validated, so the regex compiles. Skip it defensively otherwise.
        let Ok(regex) = Regex::new(pattern) else {
            continue;
        };
        let hits: Vec<String> = added
            .iter()
            .filter(|line| regex.is_match(line))
            .take(MAX_SHOWN_LINES)
            .cloned()
            .collect();
        if !hits.is_empty() {
            violations.push(Violation {
                rule: "deny_added_patterns",
                message: format!("added lines match {pattern:?}"),
                lines: hits,
            });
        }
    }

    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guards() -> Guards {
        Guards {
            deny_paths: vec!["secrets.env".into()],
            deny_added_patterns: vec!["NOSONAR".into(), r"#\s*noqa".into()],
            max_files_changed: 2,
            max_lines_changed: 10,
            forbid_test_deletion: true,
            forbid_binary_files: true,
            allow_paths: Vec::new(),
        }
    }

    fn change(path: &str, added: Option<u64>, deleted: Option<u64>, status: char) -> FileChange {
        FileChange {
            path: path.into(),
            added,
            deleted,
            status,
        }
    }

    #[test]
    fn numstat_and_name_status_are_joined() {
        let changes = parse_changes(
            "3\t1\tsrc/a.py\n-\t-\tlogo.png\n",
            "M\tsrc/a.py\nD\tlogo.png\n",
        );
        assert_eq!(changes[0], change("src/a.py", Some(3), Some(1), 'M'));
        assert_eq!(changes[1], change("logo.png", None, None, 'D'));
    }

    #[test]
    fn clean_change_has_no_violations() {
        let changes = vec![change("src/a.py", Some(3), Some(1), 'M')];
        assert!(check(&changes, &["x = 1".into()], &guards()).is_empty());
    }

    #[test]
    fn suppression_comments_in_added_lines_are_refused_with_the_lines() {
        let added = vec!["x = 1  # noqa".into(), "y = 2".into(), "# NOSONAR".into()];
        let violations = check(&[change("a.py", Some(3), Some(0), 'M')], &added, &guards());
        let hits: usize = violations
            .iter()
            .filter(|v| v.rule == "deny_added_patterns")
            .map(|v| v.lines.len())
            .sum();
        assert_eq!(hits, 2, "{violations:?}");
    }

    #[test]
    fn limits_binary_files_test_deletion_and_denied_paths_are_reported() {
        let changes = vec![
            change("a.py", Some(20), Some(0), 'M'),
            change("img.png", None, None, 'M'),
            change("tests/test_x.py", Some(0), Some(30), 'D'),
            change("secrets.env", Some(1), Some(0), 'M'),
        ];
        let rules: Vec<&str> = check(&changes, &[], &guards())
            .iter()
            .map(|v| v.rule)
            .collect();
        for expected in [
            "max_files_changed",
            "max_lines_changed",
            "forbid_binary_files",
            "forbid_test_deletion",
            "deny_paths",
        ] {
            assert!(
                rules.contains(&expected),
                "{expected} missing from {rules:?}"
            );
        }
    }

    #[test]
    fn allow_paths_refuses_anything_outside() {
        let mut g = guards();
        g.allow_paths = vec!["pyproject.toml".into(), "Dockerfile".into()];
        let changes = vec![
            change("pyproject.toml", Some(1), Some(1), 'M'),
            change("src/a.py", Some(1), Some(0), 'M'),
        ];
        let violations = check(&changes, &[], &g);
        let v = violations.iter().find(|v| v.rule == "allow_paths").unwrap();
        assert_eq!(v.lines, vec!["src/a.py".to_string()]);
    }

    #[test]
    fn added_lines_skip_file_headers() {
        let patch = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n";
        assert_eq!(added_lines(patch), vec!["new".to_string()]);
    }
}
