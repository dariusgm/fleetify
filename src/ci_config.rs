//! Pre-check of CI configuration: GitLab CI (`.gitlab-ci.yml`) and GitHub Actions
//! (`.github/workflows/*.yml` and `*.yaml`). Only structure is checked, offline and
//! in milliseconds, so a broken pipeline definition fails before a build starts.
//! Anything else is out of scope for now.

use std::fs;
use std::path::{Path, PathBuf};

use serde_yaml_ng::{Mapping, Value};

const GITLAB_FILE: &str = ".gitlab-ci.yml";
const WORKFLOWS_DIR: &str = ".github/workflows";

/// Keys of a GitLab CI file that are not jobs.
const GITLAB_KEYWORDS: &[&str] = &[
    "default",
    "include",
    "stages",
    "variables",
    "workflow",
    "image",
    "services",
    "cache",
    "before_script",
    "after_script",
];

/// One problem found in a CI file. `file` is relative to the repository root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub file: String,
    pub message: String,
}

impl Finding {
    /// One output line in the verifier's format: `E  <file>: <message>`.
    pub fn line(&self) -> String {
        format!("E  {}: {}", self.file, self.message)
    }
}

/// Checks every CI file that exists under `root`. No CI file means no findings.
pub fn check(root: &Path) -> Vec<Finding> {
    let mut findings = Vec::new();
    let gitlab = root.join(GITLAB_FILE);
    if gitlab.is_file() {
        findings.extend(check_file(root, &gitlab, check_gitlab));
    }
    for path in github_workflows(root) {
        findings.extend(check_file(root, &path, check_github));
    }
    findings
}

/// Workflow files in `.github/workflows`, sorted, without recursion.
fn github_workflows(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root.join(WORKFLOWS_DIR)) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("yml" | "yaml")
                )
        })
        .collect();
    paths.sort();
    paths
}

fn check_file(root: &Path, path: &Path, check: fn(&Value) -> Vec<String>) -> Vec<Finding> {
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string();
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            return vec![Finding {
                file: rel,
                message: format!("cannot read file: {err}"),
            }];
        }
    };
    match serde_yaml_ng::from_str::<Value>(&text) {
        Ok(Value::Null) => vec![Finding {
            file: rel,
            message: "file is empty".to_string(),
        }],
        Ok(value) => check(&value)
            .into_iter()
            .map(|message| Finding {
                file: rel.clone(),
                message,
            })
            .collect(),
        Err(err) => vec![Finding {
            file: rel,
            message: format!("invalid YAML: {err}"),
        }],
    }
}

/// Structure of a GitLab CI file: every job has a script (or inherits or triggers),
/// and a job's stage is declared in `stages` when `stages` is present.
fn check_gitlab(value: &Value) -> Vec<String> {
    let Some(top) = value.as_mapping() else {
        return vec!["top level must be a mapping".to_string()];
    };
    let stages: Option<Vec<&str>> = match top.get("stages") {
        None => None,
        Some(Value::Sequence(items)) => Some(items.iter().filter_map(Value::as_str).collect()),
        Some(_) => return vec!["stages must be a list of names".to_string()],
    };
    let mut problems = Vec::new();
    for (key, job) in top {
        let Some(name) = key.as_str() else {
            problems.push("job names must be strings".to_string());
            continue;
        };
        if name.starts_with('.') || GITLAB_KEYWORDS.contains(&name) {
            continue;
        }
        let Some(job) = job.as_mapping() else {
            problems.push(format!("job {name}: must be a mapping"));
            continue;
        };
        let inherits = job.contains_key("extends") || job.contains_key("trigger");
        if !inherits && !has_script(job) {
            problems.push(format!("job {name}: no script"));
        }
        if let (Some(stages), Some(stage)) = (&stages, job.get("stage").and_then(Value::as_str))
            && !stages.contains(&stage)
        {
            problems.push(format!("job {name}: stage {stage} is not in stages"));
        }
    }
    problems
}

fn has_script(job: &Mapping) -> bool {
    match job.get("script") {
        Some(Value::String(_)) => true,
        Some(Value::Sequence(items)) => !items.is_empty(),
        _ => false,
    }
}

/// Structure of a GitHub Actions workflow: `on` and at least one job, each job runs
/// on a runner or calls a reusable workflow, each step runs or uses an action, and
/// `needs` only names jobs that exist.
fn check_github(value: &Value) -> Vec<String> {
    let Some(top) = value.as_mapping() else {
        return vec!["top level must be a mapping".to_string()];
    };
    let mut problems = Vec::new();
    if !top.contains_key("on") && !top.contains_key(&Value::Bool(true)) {
        problems.push("missing `on` trigger".to_string());
    }
    let Some(jobs) = top.get("jobs").and_then(Value::as_mapping) else {
        problems.push("missing `jobs` mapping".to_string());
        return problems;
    };
    if jobs.is_empty() {
        problems.push("`jobs` is empty".to_string());
    }
    let names: Vec<&str> = jobs.keys().filter_map(Value::as_str).collect();
    for (key, job) in jobs {
        let name = key.as_str().unwrap_or("?");
        let Some(job) = job.as_mapping() else {
            problems.push(format!("job {name}: must be a mapping"));
            continue;
        };
        if !job.contains_key("runs-on") && !job.contains_key("uses") {
            problems.push(format!("job {name}: needs `runs-on` or `uses`"));
        }
        for needed in needs_of(job) {
            if !names.contains(&needed.as_str()) {
                problems.push(format!("job {name}: needs unknown job {needed}"));
            }
        }
        match job.get("steps") {
            None => {}
            Some(Value::Sequence(steps)) => {
                for (index, step) in steps.iter().enumerate() {
                    if let Some(problem) = check_step(step) {
                        problems.push(format!("job {name}, step {}: {problem}", index + 1));
                    }
                }
            }
            Some(_) => problems.push(format!("job {name}: steps must be a list")),
        }
    }
    problems
}

fn needs_of(job: &Mapping) -> Vec<String> {
    match job.get("needs") {
        Some(Value::String(name)) => vec![name.clone()],
        Some(Value::Sequence(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn check_step(step: &Value) -> Option<String> {
    let Some(step) = step.as_mapping() else {
        return Some("must be a mapping".to_string());
    };
    match (step.contains_key("run"), step.contains_key("uses")) {
        (false, false) => Some("needs `run` or `uses`".to_string()),
        (true, true) => Some("has both `run` and `uses`".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gitlab(text: &str) -> Vec<String> {
        check_gitlab(&serde_yaml_ng::from_str(text).unwrap())
    }

    fn github(text: &str) -> Vec<String> {
        check_github(&serde_yaml_ng::from_str(text).unwrap())
    }

    #[test]
    fn a_valid_gitlab_pipeline_has_no_findings() {
        let text = "stages: [build, test]\nbuild:\n  stage: build\n  script: make\ntest:\n  stage: test\n  script:\n    - make test\n";
        assert!(gitlab(text).is_empty());
    }

    #[test]
    fn gitlab_job_without_script_is_reported() {
        assert_eq!(
            gitlab("build:\n  stage: build\n"),
            vec!["job build: no script"]
        );
    }

    #[test]
    fn gitlab_template_and_extends_are_not_required_to_have_a_script() {
        let text =
            ".tpl:\n  image: rust\nlint:\n  extends: .tpl\ndeploy:\n  trigger:\n    project: x/y\n";
        assert!(gitlab(text).is_empty());
    }

    #[test]
    fn gitlab_stage_must_be_declared() {
        let text = "stages: [build]\ntest:\n  stage: test\n  script: make\n";
        assert_eq!(gitlab(text), vec!["job test: stage test is not in stages"]);
    }

    #[test]
    fn a_valid_github_workflow_has_no_findings() {
        let text = "on: push\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n      - run: cargo test\n";
        assert!(github(text).is_empty());
    }

    #[test]
    fn github_job_needs_a_runner_or_a_reusable_workflow() {
        let text = "on: push\njobs:\n  test:\n    steps:\n      - run: make\n";
        assert_eq!(github(text), vec!["job test: needs `runs-on` or `uses`"]);
    }

    #[test]
    fn github_step_needs_exactly_one_of_run_or_uses() {
        let text = "on: push\njobs:\n  test:\n    runs-on: x\n    steps:\n      - name: nothing\n      - run: a\n        uses: b\n";
        assert_eq!(
            github(text),
            vec![
                "job test, step 1: needs `run` or `uses`",
                "job test, step 2: has both `run` and `uses`",
            ]
        );
    }

    #[test]
    fn github_needs_must_name_existing_jobs() {
        let text = "on: push\njobs:\n  build:\n    runs-on: x\n    needs: prepare\n";
        assert_eq!(github(text), vec!["job build: needs unknown job prepare"]);
    }

    #[test]
    fn github_without_jobs_is_reported() {
        assert_eq!(github("on: push\n"), vec!["missing `jobs` mapping"]);
    }

    #[test]
    fn invalid_yaml_is_reported_with_the_file_name() {
        let dir = std::env::temp_dir().join(format!("fleetify-ci-config-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".github/workflows")).unwrap();
        fs::write(dir.join(".github/workflows/ci.yml"), "jobs: [unclosed\n").unwrap();
        let findings = check(&dir);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].file, ".github/workflows/ci.yml");
        assert!(findings[0].message.starts_with("invalid YAML"));
    }

    #[test]
    fn a_repository_without_ci_files_has_no_findings() {
        let dir = std::env::temp_dir().join(format!("fleetify-ci-none-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(check(&dir).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
