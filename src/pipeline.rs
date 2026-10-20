//! The no-agent pipeline for one repo (PLAN.md, "Run flow per repo", steps 3–9).
//! Every failure ends the repo with a result and a failure block. Nothing is
//! pushed unless the verifier and every guard passed.

use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::agents;
use crate::events::{Event, Outcome};
use crate::git::{GitContext, git};
use crate::guards;
use crate::hosts::{ChangeRequest, MergeOutcome};
use crate::report::{Failure, RepoResult};
use crate::repos::Repo;
use crate::run::{Ctx, artifacts, error_result, git_context, git_failure, probe};
use crate::runner::{CommandResult, CommandSpec, RunRequest};
use crate::state::write_atomic;
use crate::workspace::{cache_dir, create_workspace, ensure_cache};

const DEFAULT_PREPARE_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_VERIFY_TIMEOUT: Duration = Duration::from_secs(300);
/// Output lines kept in a failure block.
const MAX_TAIL: usize = 40;
/// Characters of the agent's `result.md` that go into the change request body.
const MAX_AGENT_NOTE_CHARS: usize = 4000;

/// Runs the whole pipeline for one repo.
pub fn process_repo(ctx: &Ctx<'_>, repo: &Repo) -> RepoResult {
    let key = repo.key();
    let phase = |name: &str| {
        ctx.runner.bus().emit(Event::RepoPhase {
            repo: key.clone(),
            phase: name.to_string(),
        })
    };

    phase("cloning");
    let (pattern, token) = match probe(ctx, repo) {
        Ok(found) => found,
        Err((message, rule)) => return error_result(ctx.layout, repo, message, rule, Vec::new()),
    };
    let scratch = ctx.layout.scratch_dir();
    let git_ctx = git_context(ctx, repo, &key, token, &scratch);
    let local = GitContext {
        auth: None,
        ..git_ctx
    };
    let cache = cache_dir(ctx.workdir, &repo.host, &repo.path);
    let state = match ensure_cache(&git_ctx, &repo.clone_url, &cache) {
        Ok(state) => state,
        Err(err) => return git_failure(ctx.layout, repo, err, Some(pattern)),
    };

    phase("checking");
    let facts = crate::recipe::RepoFacts {
        host_type: repo.host_type.as_str(),
        path: &repo.path,
        checkout: &cache,
    };
    let unmet = ctx.recipe.unmet_conditions(&facts);
    if !unmet.is_empty() {
        let mut result = RepoResult::new(&key, "skipped", &unmet.join("; "));
        result.rule = Some(pattern);
        phase("finished");
        return result;
    }

    let branch = ctx.recipe.branch.clone();
    let Some(client) = ctx.clients.get(&repo.host) else {
        return error_result(
            ctx.layout,
            repo,
            format!("no API client for {}", repo.host),
            Some(pattern),
            Vec::new(),
        );
    };
    // Step 3: a change request for the branch already exists, so the repo is done.
    // Without a change request there is nothing to look up; the branch check below covers it.
    let found = if ctx.recipe.delivery.open_mr {
        client.find_change_request(repo, token, &branch)
    } else {
        Ok(None)
    };
    match found {
        Ok(Some(change)) => {
            let mut result = RepoResult::new(
                &key,
                "already_done",
                "an open change request for the branch exists",
            );
            result.change_request = Some(change.url.clone());
            result.rule = Some(pattern);
            if ctx.merge {
                // The change exists already; a run with --merge still merges it.
                let expected = change.head_sha.clone();
                return merge_change(ctx, repo, token, &branch, &change, &expected, result);
            }
            phase("finished");
            return result;
        }
        Ok(None) => {}
        Err(err) => {
            return error_result(ctx.layout, repo, err.to_string(), Some(pattern), Vec::new());
        }
    }
    // Step 3, second case: the branch was pushed by an interrupted run. Open the change request only.
    let branch_ref = format!("refs/heads/{branch}");
    match git(
        &git_ctx,
        None,
        &["ls-remote", "--heads", &repo.clone_url, &branch_ref],
    ) {
        Ok(listing) if !listing.trim().is_empty() && !ctx.recipe.delivery.open_mr => {
            // Push-only: the branch is already on the remote, so an earlier run did this.
            let mut result = RepoResult::new(
                &key,
                "already_done",
                "the branch already exists on the remote",
            );
            result.rule = Some(pattern);
            phase("finished");
            return result;
        }
        Ok(listing) if !listing.trim().is_empty() => {
            phase("opening_pr");
            return open_change_request(
                ctx,
                repo,
                token,
                &state.default_branch,
                &state.base_sha,
                None,
                pattern,
                "resumed: the branch existed without a change request",
                None,
            );
        }
        Ok(_) => {}
        Err(err) => return git_failure(ctx.layout, repo, err, Some(pattern)),
    }

    // Step 4: a fresh workspace. An older workspace of this run is replaced.
    phase("preparing");
    let workspace = ctx.layout.workspace_dir(repo);
    if workspace.exists()
        && let Err(err) = fs::remove_dir_all(&workspace)
    {
        return error_result(
            ctx.layout,
            repo,
            format!("cannot remove old workspace: {err}"),
            Some(pattern),
            Vec::new(),
        );
    }
    if let Err(err) = create_workspace(
        &local,
        &cache,
        &repo.clone_url,
        &workspace,
        &state.base_sha,
        &branch,
    ) {
        return git_failure(ctx.layout, repo, err, Some(pattern));
    }

    // Step 4a: the CI configuration on the untouched base.
    if let Some(failed) = ci_pre_check(ctx, repo, &pattern, &workspace, "baseline") {
        return failed;
    }

    // Step 4b: the verifier on the untouched base. A broken base is not caused by the
    // change, so the repo stops here, before prepare or any agent runs.
    if let Some(verify) = ctx.recipe.verify.as_ref()
        && verify.baseline
        && let Some(spec) = ctx.recipe.verify_spec()
    {
        phase("baseline");
        let timeout = verify
            .timeout_s
            .map_or(DEFAULT_VERIFY_TIMEOUT, Duration::from_secs);
        let tail_lines = verify.tail_lines.map_or(40, |n| n as usize).min(MAX_TAIL);
        match run_step(ctx, repo, &spec, &workspace, timeout) {
            Err(message) => {
                return error_result(ctx.layout, repo, message, Some(pattern), Vec::new());
            }
            Ok(result) if !result.is_success() => {
                let tail = failure_lines(&result, &verify.error_patterns, tail_lines);
                let mut failed = step_failure(ctx, repo, "baseline verify", &result, pattern, tail);
                failed.result = "base_broken".to_string();
                if let Some(block) = failed.failure.as_mut() {
                    block.class = "base_broken".to_string();
                }
                return failed;
            }
            Ok(_) => {}
        }
    }

    // Step 5: the recipe's prepare step.
    if let Some(spec) = ctx.recipe.prepare_spec() {
        let timeout = ctx
            .recipe
            .prepare
            .as_ref()
            .and_then(|t| t.timeout_s)
            .map_or(DEFAULT_PREPARE_TIMEOUT, Duration::from_secs);
        match run_step(ctx, repo, &spec, &workspace, timeout) {
            Err(message) => {
                return error_result(ctx.layout, repo, message, Some(pattern), Vec::new());
            }
            Ok(result) if !result.is_success() => {
                return step_failure(ctx, repo, "prepare", &result, pattern, Vec::new());
            }
            Ok(_) => {}
        }
    }

    // Step 5b: the agent, when the recipe has instructions. It edits the prepared
    // workspace; the verifier and the guards below judge what it changed.
    let mut agent_note: Option<String> = None;
    if let Some(name) = ctx.agent {
        phase("agent");
        match run_agent(ctx, repo, name, &workspace) {
            Err(message) => {
                return error_result(ctx.layout, repo, message, Some(pattern), Vec::new());
            }
            Ok(result) if !result.is_success() => {
                let mut failed = step_failure(ctx, repo, "agent", &result, pattern, Vec::new());
                let (result_name, class) = match result.outcome {
                    Outcome::TimedOut => ("agent_timeout", "agent_timeout"),
                    Outcome::WaitingForInput => ("needs_input", "needs_input"),
                    _ => ("error", "error"),
                };
                failed.result = result_name.to_string();
                if let Some(block) = failed.failure.as_mut() {
                    block.class = class.to_string();
                }
                return failed;
            }
            Ok(_) => {}
        }
        // The agent may have committed by itself. Move the branch back to the base so
        // that the diff below is the whole change and the orchestrator makes the one commit.
        if let Err(err) = git(
            &local,
            Some(&workspace),
            &["reset", "--soft", &state.base_sha],
        ) {
            return git_failure(ctx.layout, repo, err, Some(pattern));
        }
        if let Some(lines) = blocked_lines(&workspace) {
            let mut blocked = error_result(
                ctx.layout,
                repo,
                "the agent reported that it is blocked".to_string(),
                Some(pattern),
                lines,
            );
            blocked.result = "needs_input".to_string();
            if let Some(block) = blocked.failure.as_mut() {
                block.class = "needs_input".to_string();
            }
            phase("finished");
            return blocked;
        }
        agent_note = read_agent_note(ctx, &workspace);
    }

    // Step 6: is there anything to commit?
    if let Err(err) = git(&local, Some(&workspace), &["add", "-A"]) {
        return git_failure(ctx.layout, repo, err, Some(pattern));
    }
    let staged = match git(
        &local,
        Some(&workspace),
        &["diff", "--cached", "--name-only"],
    ) {
        Ok(names) => names,
        Err(err) => return git_failure(ctx.layout, repo, err, Some(pattern)),
    };
    if staged.trim().is_empty() {
        let mut result = RepoResult::new(&key, "no_changes", "the prepare step changed nothing");
        result.rule = Some(pattern);
        phase("finished");
        return result;
    }

    // Step 7a: the CI configuration after the change, before the build.
    if let Some(failed) = ci_pre_check(ctx, repo, &pattern, &workspace, "change") {
        return failed;
    }

    // Step 8, first part: the verifier.
    phase("verifying");
    if let Some(verify) = ctx.recipe.verify.as_ref()
        && let Some(spec) = ctx.recipe.verify_spec()
    {
        let timeout = verify
            .timeout_s
            .map_or(DEFAULT_VERIFY_TIMEOUT, Duration::from_secs);
        let tail_lines = verify.tail_lines.map_or(40, |n| n as usize).min(MAX_TAIL);
        match run_step(ctx, repo, &spec, &workspace, timeout) {
            Err(message) => {
                return error_result(ctx.layout, repo, message, Some(pattern), Vec::new());
            }
            Ok(result) if !result.is_success() => {
                let tail = failure_lines(&result, &verify.error_patterns, tail_lines);
                let mut failed = step_failure(ctx, repo, "verify", &result, pattern, tail);
                failed.result = "verify_failed".to_string();
                if let Some(block) = failed.failure.as_mut() {
                    block.class = "verify_failed".to_string();
                }
                return failed;
            }
            Ok(_) => {}
        }
    }

    // Step 8, second part: the guards, on the staged diff.
    phase("guards");
    let numstat = match git(&local, Some(&workspace), &["diff", "--cached", "--numstat"]) {
        Ok(text) => text,
        Err(err) => return git_failure(ctx.layout, repo, err, Some(pattern)),
    };
    let name_status = match git(
        &local,
        Some(&workspace),
        &["diff", "--cached", "--name-status"],
    ) {
        Ok(text) => text,
        Err(err) => return git_failure(ctx.layout, repo, err, Some(pattern)),
    };
    let patch = match git(
        &local,
        Some(&workspace),
        &["diff", "--cached", "--no-color"],
    ) {
        Ok(text) => text,
        Err(err) => return git_failure(ctx.layout, repo, err, Some(pattern)),
    };
    let patch_path = ctx.layout.repo_dir(repo).join("diff.patch");
    if let Err(err) = write_atomic(&patch_path, patch.as_bytes()) {
        tracing::warn!(error = %err, "cannot write diff.patch");
    }
    let changes = guards::parse_changes(&numstat, &name_status);
    let violations = guards::check(&changes, &guards::added_lines(&patch), &ctx.recipe.guards);
    if !violations.is_empty() {
        let mut result = RepoResult::new(&key, "guard_violation", &violations[0].message);
        result.rule = Some(pattern.clone());
        let tail: Vec<String> = violations
            .iter()
            .flat_map(|v| {
                std::iter::once(format!("{}: {}", v.rule, v.message))
                    .chain(v.lines.iter().map(|l| format!("  {l}")))
            })
            .take(MAX_TAIL)
            .collect();
        let mut artifacts = artifacts(ctx.layout, repo);
        artifacts.insert("diff".to_string(), patch_path.display().to_string());
        result.failure = Some(Failure {
            class: "guard_violation".to_string(),
            message: violations
                .iter()
                .map(|v| v.message.clone())
                .collect::<Vec<_>>()
                .join("; "),
            rule: Some(violations[0].rule.to_string()),
            output_tail: tail,
            artifacts,
        });
        phase("finished");
        return result;
    }

    // Step 9a: one commit, pushed through the explicit URL.
    phase("committing");
    let message = ctx
        .recipe
        .commit_message
        .clone()
        .unwrap_or_else(|| ctx.recipe.pr_title.clone());
    // The repo's own identity wins; without one, the commit is attributed to fleetify.
    let identity_set = git(&local, Some(&workspace), &["config", "--get", "user.email"])
        .is_ok_and(|email| !email.trim().is_empty());
    let mut commit_args: Vec<&str> = Vec::new();
    if !identity_set {
        commit_args.extend([
            "-c",
            "user.name=fleetify",
            "-c",
            "user.email=fleetify@localhost",
        ]);
    }
    commit_args.extend(["commit", "--quiet", "-m", &message]);
    if let Err(err) = git(&local, Some(&workspace), &commit_args) {
        return git_failure(ctx.layout, repo, err, Some(pattern));
    }
    phase("pushing");
    let refspec = format!("HEAD:refs/heads/{branch}");
    if let Err(err) = git(
        &git_ctx,
        Some(&workspace),
        &["push", "--quiet", &repo.clone_url, &refspec],
    ) {
        return git_failure(ctx.layout, repo, err, Some(pattern));
    }
    let pushed_sha = match git(&local, Some(&workspace), &["rev-parse", "HEAD"]) {
        Ok(sha) => sha.trim().to_string(),
        Err(err) => return git_failure(ctx.layout, repo, err, Some(pattern)),
    };

    // Push-only: the branch is pushed and nothing else happens.
    if !ctx.recipe.delivery.open_mr {
        let mut result = RepoResult::new(
            &key,
            "pushed",
            &format!("pushed {branch} at {}", short(&pushed_sha)),
        );
        result.rule = Some(pattern);
        phase("finished");
        return result;
    }

    // Step 9b: the change request.
    phase("opening_pr");
    let mut result = open_change_request(
        ctx,
        repo,
        token,
        &state.default_branch,
        &state.base_sha,
        Some(&pushed_sha),
        pattern,
        &format!("pushed {branch} at {}", short(&pushed_sha)),
        agent_note.as_deref(),
    );
    if result.successful {
        result.result = "pr_open".to_string();
        result.successful = crate::report::is_successful("pr_open");
        if ctx.merge {
            let client = ctx.clients.get(&repo.host).expect("client checked above");
            match client.find_change_request(repo, token, &branch) {
                Ok(Some(change)) => {
                    result = merge_change(ctx, repo, token, &branch, &change, &pushed_sha, result);
                }
                Ok(None) => {
                    result.result = "merge_blocked".to_string();
                    result.successful = false;
                    result.detail = "no open change request to merge".to_string();
                }
                Err(err) => {
                    return error_result(
                        ctx.layout,
                        repo,
                        err.to_string(),
                        result.rule.clone(),
                        Vec::new(),
                    );
                }
            }
        }
    }
    phase("finished");
    result
}

/// Merges an open change request, for a run started with `--merge`. The merge rule for
/// the repository decides whether that is allowed, and the host refuses unless the head is
/// still `expected`, the commit fleetify pushed. A successful merge can delete the
/// source branch when the recipe asks for it.
#[allow(clippy::too_many_arguments)]
fn merge_change(
    ctx: &Ctx<'_>,
    repo: &Repo,
    token: &crate::credentials::Secret,
    branch: &str,
    change: &ChangeRequest,
    expected: &str,
    mut result: RepoResult,
) -> RepoResult {
    let Some(client) = ctx.clients.get(&repo.host) else {
        return error_result(
            ctx.layout,
            repo,
            format!("no API client for {}", repo.host),
            result.rule.clone(),
            Vec::new(),
        );
    };
    ctx.runner.bus().emit(Event::RepoPhase {
        repo: repo.key(),
        phase: "merging".to_string(),
    });
    // Without a CI gate, only a merge rule for this repository allows the merge.
    let rule = ctx.config.merge_rule(&repo.key());
    if !rule.is_some_and(|rule| rule.merge_without_ci) {
        result.result = "merge_blocked".to_string();
        result.successful = false;
        result.detail = format!(
            "no [[merge_policy]] rule in config.toml allows merging {} without a CI gate",
            repo.key()
        );
        return result;
    }
    match client.merge(repo, token, change, expected) {
        Ok(MergeOutcome::Merged) => {
            result.result = "merged".to_string();
            result.successful = crate::report::is_successful("merged");
            result.detail = format!("merged {} at {}", change.url, short(expected));
        }
        Ok(MergeOutcome::Blocked(reason)) => {
            result.result = "merge_blocked".to_string();
            result.successful = false;
            result.detail = reason;
            return result;
        }
        Err(err) => {
            return error_result(
                ctx.layout,
                repo,
                err.to_string(),
                result.rule.clone(),
                Vec::new(),
            );
        }
    }
    if ctx.recipe.delivery.delete_branch && rule.is_some_and(|rule| rule.delete_branch) {
        match client.delete_branch(repo, token, branch) {
            Ok(()) => result.detail.push_str("; source branch deleted"),
            Err(err) => result
                .detail
                .push_str(&format!("; source branch not deleted: {err}")),
        }
    }
    result
}

/// Opens the change request and builds the result. `pushed` is the commit that
/// was pushed, if this run pushed one.
#[allow(clippy::too_many_arguments)]
fn open_change_request(
    ctx: &Ctx<'_>,
    repo: &Repo,
    token: &crate::credentials::Secret,
    default_branch: &str,
    base_sha: &str,
    pushed: Option<&str>,
    pattern: String,
    detail: &str,
    agent_note: Option<&str>,
) -> RepoResult {
    let key = repo.key();
    let Some(client) = ctx.clients.get(&repo.host) else {
        return error_result(
            ctx.layout,
            repo,
            format!("no API client for {}", repo.host),
            Some(pattern),
            Vec::new(),
        );
    };
    let branch = &ctx.recipe.branch;
    let body = change_body(ctx, base_sha, default_branch, agent_note);
    match client.create_change_request(
        repo,
        token,
        branch,
        default_branch,
        &ctx.recipe.pr_title,
        &body,
    ) {
        Ok(ChangeRequest { url, .. }) => {
            let mut result = RepoResult::new(&key, "pr_open", detail);
            result.change_request = Some(url);
            result.pushed_sha = pushed.map(str::to_string);
            result.rule = Some(pattern);
            result
        }
        Err(err) => {
            let mut result = error_result(
                ctx.layout,
                repo,
                format!("cannot open the change request: {err}"),
                Some(pattern),
                Vec::new(),
            );
            result.pushed_sha = pushed.map(str::to_string);
            result
        }
    }
}

/// The change request body: what the run did, not what the agent or the host wants.
fn change_body(
    ctx: &Ctx<'_>,
    base_sha: &str,
    default_branch: &str,
    agent_note: Option<&str>,
) -> String {
    let mut body = format!(
        "Automated change by fleetify.\n\n\
         - Recipe: {}\n\
         - Run: {}\n\
         - Base: {default_branch} at {}\n\
         - Verifier: {}\n\
         - Guards: passed\n\n\
         Review the diff before merging.",
        ctx.recipe.name,
        ctx.layout.run_id,
        short(base_sha),
        if ctx.recipe.verify.is_some() {
            "passed"
        } else {
            "not configured"
        },
    );
    if let Some(note) = agent_note {
        // The agent wrote this, nobody reviewed it: say so, and quote it.
        body.push_str("\n\n### Agent notes (written by the agent, not reviewed)\n\n");
        for line in note.lines() {
            body.push_str(&format!("> {line}\n"));
        }
    }
    body
}

/// The agent's own report from `.agent/result.md`, redacted and length-limited.
fn read_agent_note(ctx: &Ctx<'_>, workspace: &Path) -> Option<String> {
    let text = fs::read_to_string(workspace.join(agents::RESULT_FILE)).ok()?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let limited: String = text.chars().take(MAX_AGENT_NOTE_CHARS).collect();
    Some(ctx.runner.redactor().redact(&limited))
}

/// The CI configuration pre-check. A broken file stops the repo before the build:
/// on the base it is `base_broken`, after prepare it is `ci_invalid`.
fn ci_pre_check(
    ctx: &Ctx<'_>,
    repo: &Repo,
    pattern: &str,
    workspace: &Path,
    stage: &str,
) -> Option<RepoResult> {
    if !ctx.recipe.ci.validate {
        return None;
    }
    let findings = crate::ci_config::check(workspace);
    if findings.is_empty() {
        return None;
    }
    let name = if stage == "baseline" {
        "base_broken"
    } else {
        "ci_invalid"
    };
    let message = format!(
        "{stage}: CI configuration is invalid ({} problem(s))",
        findings.len()
    );
    let tail: Vec<String> = findings.iter().map(|f| f.line()).collect();
    let mut failed = error_result(ctx.layout, repo, message, Some(pattern.to_string()), tail);
    failed.result = name.to_string();
    if let Some(block) = failed.failure.as_mut() {
        block.class = name.to_string();
    }
    Some(failed)
}

/// Renders the step for this OS and runs it in the workspace.
fn run_step(
    ctx: &Ctx<'_>,
    repo: &Repo,
    spec: &CommandSpec,
    workspace: &Path,
    timeout: Duration,
) -> Result<CommandResult, String> {
    let workspace_arg = workspace.display().to_string();
    let tools_arg = ctx.tools.display().to_string();
    let argv = spec
        .render(
            cfg!(windows),
            &[("repo", &workspace_arg), ("tools", &tools_arg)],
        )
        .map_err(|err| err.to_string())?;
    let mut request = RunRequest::new(repo.key(), argv);
    request.cwd = Some(workspace.to_path_buf());
    request.timeout = Some(timeout);
    Ok(ctx.runner.run(request))
}

/// Writes the task and attachments into `.agent/`, then runs the agent in the workspace.
fn run_agent(
    ctx: &Ctx<'_>,
    repo: &Repo,
    name: &str,
    workspace: &Path,
) -> Result<CommandResult, String> {
    agents::write_task(workspace, ctx.recipe_dir, ctx.recipe)
        .map_err(|err| format!("cannot write the agent task: {err}"))?;
    let invocation = agents::invocation(name, ctx.recipe, &ctx.config.permissions)?;
    let grants = agents::grants(ctx.recipe, &ctx.config.permissions, name);
    ctx.runner.bus().emit(Event::PermissionsEffective {
        repo: repo.key(),
        agent: name.to_string(),
        commands: grants.commands,
        web: grants.web,
        directories: grants.directories,
        unrestricted: grants.unrestricted,
    });
    let mut request = RunRequest::new(repo.key(), invocation.argv);
    request.cwd = Some(workspace.to_path_buf());
    request.timeout = Some(Duration::from_secs(
        u64::from(ctx.recipe.limits.agent_timeout_min) * 60,
    ));
    let idle_min = ctx.recipe.limits.idle_warn_min;
    request.idle_warn = (idle_min > 0).then(|| Duration::from_secs(u64::from(idle_min) * 60));
    request.prompt_check = Some(agents::wants_input);
    request.env = invocation.env;
    Ok(ctx.runner.run(request))
}

/// The lines of `blocked.md` when the agent left one in the workspace.
fn blocked_lines(workspace: &Path) -> Option<Vec<String>> {
    let text = fs::read_to_string(workspace.join(agents::BLOCKED_FILE)).ok()?;
    Some(text.lines().take(MAX_TAIL).map(str::to_string).collect())
}

fn step_failure(
    ctx: &Ctx<'_>,
    repo: &Repo,
    step: &str,
    result: &CommandResult,
    pattern: String,
    tail: Vec<String>,
) -> RepoResult {
    let reason = match &result.outcome {
        Outcome::Exited { code: Some(code) } => format!("exited with {code}"),
        Outcome::Exited { code: None } => "was killed by a signal".to_string(),
        Outcome::TimedOut => "timed out".to_string(),
        other => format!("{other:?}"),
    };
    let tail = if tail.is_empty() {
        tail_of(result)
    } else {
        tail
    };
    let message = format!("{step} step {reason}");
    let mut failed = error_result(ctx.layout, repo, message, Some(pattern), tail);
    failed.result = "error".to_string();
    failed
}

/// The last lines of a command's output, both streams.
fn tail_of(result: &CommandResult) -> Vec<String> {
    let lines: Vec<String> = result.lines.iter().map(|l| l.text.clone()).collect();
    lines[lines.len().saturating_sub(MAX_TAIL)..].to_vec()
}

/// Lines matching one of the error patterns, or the last `n` lines when none match.
fn failure_lines(result: &CommandResult, patterns: &[String], n: usize) -> Vec<String> {
    let regexes: Vec<regex::Regex> = patterns
        .iter()
        .filter_map(|p| regex::Regex::new(p).ok())
        .collect();
    let lines: Vec<String> = result.lines.iter().map(|l| l.text.clone()).collect();
    let matching: Vec<String> = lines
        .iter()
        .filter(|l| regexes.iter().any(|r| r.is_match(l)))
        .cloned()
        .collect();
    let chosen = if matching.is_empty() { lines } else { matching };
    chosen[chosen.len().saturating_sub(n)..].to_vec()
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}
