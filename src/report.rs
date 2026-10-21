//! Results, the run report (`report.json`) and its JSON schema (PLAN.md,
//! "Events, renderers and the report"). Every text in a report is redacted and
//! length-limited. A non-successful repo always carries a `failure` block with a
//! non-empty message.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::events::Redactor;

/// Version of the report format.
pub const REPORT_SCHEMA: u32 = 1;
const MAX_MESSAGE_CHARS: usize = 2000;
const MAX_TAIL_LINES: usize = 40;
const MAX_LINE_CHARS: usize = 300;

/// Results that count as success (PLAN.md, "Successful results"). `dry_run` counts
/// as success because a dry run has no failures to report, only plans.
pub fn is_successful(result: &str) -> bool {
    matches!(
        result,
        "pushed"
            | "pr_open"
            | "ci_passed"
            | "merged"
            | "skipped"
            | "already_done"
            | "no_changes"
            | "dry_run"
    )
}

/// Why a repo is not successful, with what a caller needs to act on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Failure {
    /// The result name, e.g. `error` or `interrupted`.
    pub class: String,
    pub message: String,
    /// The credential rule or `applies_to` condition involved, if any.
    pub rule: Option<String>,
    pub output_tail: Vec<String>,
    /// Paths to the logs and artifacts of this repo.
    pub artifacts: BTreeMap<String, String>,
}

/// One repo's result. Also the line format of `results.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RepoResult {
    /// `host/path`.
    pub repo: String,
    pub result: String,
    pub successful: bool,
    /// Human-readable summary of what happened or would happen.
    pub detail: String,
    /// The credential rule that applied (its pattern, never a token).
    pub rule: Option<String>,
    pub change_request: Option<String>,
    pub pushed_sha: Option<String>,
    /// The model the agent ran with, when the agent ran and a model was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub failure: Option<Failure>,
}

impl RepoResult {
    pub fn new(repo: &str, result: &str, detail: &str) -> Self {
        RepoResult {
            repo: repo.to_string(),
            result: result.to_string(),
            successful: is_successful(result),
            detail: detail.to_string(),
            rule: None,
            change_request: None,
            pushed_sha: None,
            model: None,
            failure: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Summary {
    pub total: usize,
    pub successful: usize,
    pub by_result: BTreeMap<String, usize>,
    /// Repos that were not started because the run stopped after a wave.
    #[serde(default)]
    pub pending: usize,
}

/// The report of one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Report {
    pub schema: u32,
    pub run_id: String,
    pub recipe: String,
    /// RFC 3339, UTC.
    pub started: String,
    /// RFC 3339, UTC. Empty while the run is still going.
    pub finished: String,
    pub exit_code: i32,
    pub summary: Summary,
    pub repos: Vec<RepoResult>,
}

/// Builds the report. Text is redacted and cut to the plan's limits. Missing
/// failure blocks are added for every unsuccessful repo.
pub fn build(
    run_id: &str,
    recipe: &str,
    started: &str,
    finished: &str,
    results: Vec<RepoResult>,
    redactor: &Redactor,
) -> Report {
    let repos: Vec<RepoResult> = results
        .into_iter()
        .map(|mut result| {
            result.detail = clean_text(redactor, &result.detail, MAX_MESSAGE_CHARS);
            result.rule = result
                .rule
                .map(|r| clean_text(redactor, &r, MAX_LINE_CHARS));
            result.failure = if result.successful {
                None
            } else {
                Some(clean_failure(redactor, result.failure.take(), &result))
            };
            result
        })
        .collect();

    let mut by_result: BTreeMap<String, usize> = BTreeMap::new();
    for repo in &repos {
        *by_result.entry(repo.result.clone()).or_insert(0) += 1;
    }
    let successful = repos.iter().filter(|r| r.successful).count();
    let exit_code = i32::from(successful != repos.len());

    Report {
        schema: REPORT_SCHEMA,
        run_id: run_id.to_string(),
        recipe: recipe.to_string(),
        started: started.to_string(),
        finished: finished.to_string(),
        exit_code,
        summary: Summary {
            total: repos.len(),
            successful,
            by_result,
            pending: 0,
        },
        repos,
    }
}

fn clean_failure(redactor: &Redactor, failure: Option<Failure>, result: &RepoResult) -> Failure {
    let mut failure = failure.unwrap_or(Failure {
        class: result.result.clone(),
        message: String::new(),
        rule: None,
        output_tail: Vec::new(),
        artifacts: BTreeMap::new(),
    });
    if failure.message.trim().is_empty() {
        failure.message = if result.detail.trim().is_empty() {
            format!("{} without details", result.result)
        } else {
            result.detail.clone()
        };
    }
    failure.class = clean_text(redactor, &failure.class, MAX_LINE_CHARS);
    failure.message = clean_text(redactor, &failure.message, MAX_MESSAGE_CHARS);
    failure.rule = failure
        .rule
        .map(|r| clean_text(redactor, &r, MAX_LINE_CHARS));
    let skip = failure.output_tail.len().saturating_sub(MAX_TAIL_LINES);
    failure.output_tail = failure
        .output_tail
        .iter()
        .skip(skip)
        .map(|line| clean_text(redactor, line, MAX_LINE_CHARS))
        .collect();
    failure.artifacts = failure
        .artifacts
        .into_iter()
        .map(|(k, v)| (k, clean_text(redactor, &v, MAX_LINE_CHARS)))
        .collect();
    failure
}

/// Redacts secrets and cuts text to `max` characters, marking the cut.
fn clean_text(redactor: &Redactor, text: &str, max: usize) -> String {
    let redacted = redactor.redact(text);
    if redacted.chars().count() <= max {
        return redacted;
    }
    let mut cut: String = redacted.chars().take(max).collect();
    cut.push_str(" [truncated]");
    cut
}

/// Serialises appends from parallel workers.
static APPEND_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// [`append_result`] for concurrent callers.
pub fn append_result_locked(path: &Path, result: &RepoResult) -> io::Result<()> {
    let _guard = APPEND_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    append_result(path, result)
}

/// Appends one result to `results.jsonl`. The last line for a repo counts.
pub fn append_result(path: &Path, result: &RepoResult) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(result).map_err(io::Error::other)?;
    writeln!(file, "{line}")
}

/// Reads `results.jsonl`: the last line per repo wins. Unreadable lines are skipped.
pub fn read_results(path: &Path) -> io::Result<Vec<RepoResult>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut order: Vec<String> = Vec::new();
    let mut latest: BTreeMap<String, RepoResult> = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(result) = serde_json::from_str::<RepoResult>(line) {
            if !latest.contains_key(&result.repo) {
                order.push(result.repo.clone());
            }
            latest.insert(result.repo.clone(), result);
        }
    }
    Ok(order
        .into_iter()
        .filter_map(|r| latest.remove(&r))
        .collect())
}

/// Renders a report as a table. With `failed_only`, successful repos are left out.
pub fn render_table(report: &Report, failed_only: bool) -> String {
    let rows: Vec<&RepoResult> = report
        .repos
        .iter()
        .filter(|r| !failed_only || !r.successful)
        .collect();
    let mut out = format!("run {} ({})\n", report.run_id, report.recipe);
    let repo_width = rows.iter().map(|r| r.repo.len()).max().unwrap_or(4).max(4);
    let result_width = rows
        .iter()
        .map(|r| r.result.len())
        .max()
        .unwrap_or(6)
        .max(6);
    for row in &rows {
        let detail = row
            .failure
            .as_ref()
            .map(|f| f.message.as_str())
            .unwrap_or(row.detail.as_str());
        out.push_str(&format!(
            "{:<repo_width$}  {:<result_width$}  {}\n",
            row.repo, row.result, detail
        ));
    }
    let by_result: Vec<String> = report
        .summary
        .by_result
        .iter()
        .map(|(result, count)| format!("{count} {result}"))
        .collect();
    out.push_str(&format!(
        "{} of {} successful ({})\n",
        report.summary.successful,
        report.summary.total,
        by_result.join(", ")
    ));
    out
}

/// JSON Schema of `report.json` for this binary.
pub fn json_schema() -> String {
    serde_json::to_string_pretty(&schemars::schema_for!(Report)).expect("schema serialises")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redactor() -> Redactor {
        Redactor::new(["secret-token-123"])
    }

    #[test]
    fn success_set_matches_the_plan() {
        for ok in [
            "pr_open",
            "ci_passed",
            "merged",
            "skipped",
            "already_done",
            "no_changes",
            "dry_run",
        ] {
            assert!(is_successful(ok), "{ok}");
        }
        for bad in [
            "error",
            "verify_failed",
            "base_broken",
            "interrupted",
            "ci_failed",
            "needs_input",
            "api_limit",
            "rate_limited",
        ] {
            assert!(!is_successful(bad), "{bad}");
        }
    }

    #[test]
    fn unsuccessful_repos_always_get_a_failure_block_with_a_message() {
        let report = build(
            "r",
            "x",
            "t0",
            "t1",
            vec![RepoResult::new("github.com/a/b", "error", "")],
            &redactor(),
        );
        let failure = report.repos[0].failure.as_ref().unwrap();
        assert!(!failure.message.is_empty());
        assert_eq!(failure.class, "error");
        assert_eq!(report.exit_code, 1);
    }

    #[test]
    fn secrets_are_redacted_and_long_text_is_cut() {
        let long = "x".repeat(MAX_MESSAGE_CHARS + 50);
        let mut result = RepoResult::new(
            "github.com/a/b",
            "error",
            &format!("token secret-token-123 {long}"),
        );
        result.failure = Some(Failure {
            class: "error".into(),
            message: "bad secret-token-123".into(),
            rule: None,
            output_tail: vec!["line secret-token-123".into()],
            artifacts: BTreeMap::new(),
        });
        let report = build("r", "x", "t0", "", vec![result], &redactor());
        let text = serde_json::to_string(&report).unwrap();
        assert!(!text.contains("secret-token-123"), "{text}");
        assert!(report.repos[0].detail.ends_with("[truncated]"));
    }

    #[test]
    fn last_result_per_repo_wins_and_order_is_kept() {
        let dir =
            std::env::temp_dir().join(format!("fleetify-results-unit-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("results.jsonl");
        let _ = fs::remove_file(&path);
        append_result(&path, &RepoResult::new("a/one", "error", "first")).unwrap();
        append_result(&path, &RepoResult::new("b/two", "dry_run", "ok")).unwrap();
        append_result(&path, &RepoResult::new("a/one", "dry_run", "second")).unwrap();
        let results = read_results(&path).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].repo, "a/one");
        assert_eq!(results[0].detail, "second");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn table_lists_repos_and_summary() {
        let report = build(
            "run-1",
            "rec",
            "t0",
            "t1",
            vec![
                RepoResult::new("github.com/a/b", "dry_run", "would branch"),
                RepoResult::new("github.com/c/d", "skipped", "host_type is gitlab"),
            ],
            &redactor(),
        );
        let table = render_table(&report, false);
        assert!(table.contains("github.com/a/b"));
        assert!(table.contains("2 of 2 successful"), "{table}");
        let failed = render_table(&report, true);
        assert!(!failed.contains("github.com/a/b"));
    }
}
