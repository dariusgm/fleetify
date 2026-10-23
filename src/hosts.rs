//! Host layer: GitHub and GitLab over their REST APIs (docs/design/host-layer.md, "Host layer").
//! Everything host-specific sits behind [`Host`]. Tokens are passed per call and
//! sent only in request headers, never in URLs. CI is not part of this trait.

pub mod github;
pub mod gitlab;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::Method;
use reqwest::blocking::Client;
use reqwest::header::HeaderMap;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::credentials::Secret;
use crate::events::{Event, EventBus, WeakEventBus};
use crate::repos::Repo;

/// Timeout for one API call.
const API_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest error body kept in an error message.
const MAX_BODY_CHARS: usize = 300;

/// Kind of Git host. Decides which REST API and CI integration are used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HostType {
    Github,
    Gitlab,
}

impl HostType {
    pub fn as_str(self) -> &'static str {
        match self {
            HostType::Github => "github",
            HostType::Gitlab => "gitlab",
        }
    }
}

impl fmt::Display for HostType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Maps host names to host kinds. A repository's host comes from its URL, and only hosts in
/// this table are accepted in `repos.txt`. The table holds `github.com` and `gitlab.com`,
/// plus the hosts the recipe declares in `[hosts."<name>"]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostTable {
    hosts: BTreeMap<String, HostType>,
}

impl HostTable {
    pub fn with_builtins() -> Self {
        let mut hosts = BTreeMap::new();
        hosts.insert("github.com".to_string(), HostType::Github);
        hosts.insert("gitlab.com".to_string(), HostType::Gitlab);
        HostTable { hosts }
    }

    /// Adds a declared host. A built-in host keeps its kind; declaring it as another kind is
    /// an error.
    pub fn declare(&mut self, name: &str, kind: HostType) -> Result<(), String> {
        if !is_valid_host_name(name) {
            return Err(format!(
                "host name {name:?} must be lowercase letters, digits, '.' or '-' (no scheme, path or port)"
            ));
        }
        match self.get(name) {
            Some(existing) if existing != kind => Err(format!(
                "host {name} is built in as {existing} and cannot be declared as {kind}"
            )),
            _ => {
                self.hosts.insert(name.to_string(), kind);
                Ok(())
            }
        }
    }

    /// Lookup is case-insensitive.
    pub fn get(&self, host: &str) -> Option<HostType> {
        self.hosts.get(&host.to_ascii_lowercase()).copied()
    }
}

/// Lowercase letters, digits, `.` and `-`, with at least one letter or digit.
pub fn is_valid_host_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-')
}

/// Outcome of a read-access probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Ok,
    Unauthorized,
    Forbidden,
    NotFound,
}

impl Access {
    pub fn describe(self) -> &'static str {
        match self {
            Access::Ok => "read access ok",
            Access::Unauthorized => "401 unauthorized: the token was rejected",
            Access::Forbidden => "403 forbidden: the token may not read this repository",
            Access::NotFound => {
                "404 not found: the repository does not exist or the token cannot see it"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrState {
    Open,
    Merged,
    Closed,
}

/// A pull request (GitHub) or merge request (GitLab).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeRequest {
    pub url: String,
    /// PR number or MR internal id.
    pub id: String,
    pub head_sha: String,
    pub state: CrState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    Merged,
    /// The host refused, with its reason.
    Blocked(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// The API answered with an unexpected status.
    Status { status: u16, message: String },
    /// No answer at all: DNS, connection, TLS or timeout.
    Transport(String),
    /// The answer did not have the expected shape.
    Parse(String),
    /// The host cannot do this yet.
    Unsupported(String),
    /// The host asked us to hold back for `wait`, and that is longer than the configured
    /// ceiling. Not waited for; the repo ends as `rate_limited`.
    RateLimited { wait: Duration, reason: String },
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostError::Status { status, message } => write!(f, "HTTP {status}: {message}"),
            HostError::Transport(message) => write!(f, "cannot reach the host: {message}"),
            HostError::Parse(message) => write!(f, "unexpected answer from the host: {message}"),
            HostError::Unsupported(message) => write!(f, "not supported yet: {message}"),
            HostError::RateLimited { wait, reason } => write!(
                f,
                "rate limited ({reason}); the host allows requests again in {} s",
                wait.as_secs()
            ),
        }
    }
}

impl std::error::Error for HostError {}

impl HostError {
    /// A failure that may pass on its own: no answer at all, or the host is overloaded
    /// (429 or 5xx). Such a failure is retried, and ends a repo as `host_unreachable`
    /// when it outlasts the budget. Every other error is final.
    pub fn is_transient(&self) -> bool {
        match self {
            HostError::Transport(_) => true,
            HostError::Status { status, .. } => *status == 429 || *status >= 500,
            HostError::Parse(_) | HostError::Unsupported(_) | HostError::RateLimited { .. } => {
                false
            }
        }
    }

    /// A rate limit longer than the configured ceiling.
    pub fn is_rate_limited(&self) -> bool {
        matches!(self, HostError::RateLimited { .. })
    }
}

/// Everything host-specific. Methods that talk to the API take the token that
/// applies to the repo (from the credential rules).
/// The state of one check run or commit status, as the host reports it for a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiItem {
    pub name: String,
    pub state: CiState,
}

/// At most this many failed jobs are handed to the agent; the first failures come first.
pub const MAX_CI_FAILURES: usize = 5;

/// How much of one failed job's log reaches the agent: the last `lines` lines, and at most
/// `chars` characters, whichever is smaller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogBudget {
    pub lines: usize,
    pub chars: usize,
}

/// One failed job of a commit's CI and the part of its log that fits the budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiFailure {
    /// `workflow / job` on GitHub, the job name on GitLab (with its pipeline when nested).
    pub name: String,
    pub log_tail: Vec<String>,
}

/// The part of a GitHub job log that belongs to its failed step, within `budget`. The step
/// ends at its `##[error]` line; its section starts at the last `##[group]` line before it.
/// Steps after the failed one (post steps, cleanup) are left out. Without an error line the
/// tail of the whole log is used.
pub fn failed_step_log(text: &str, budget: LogBudget) -> Vec<String> {
    let lines: Vec<&str> = text.lines().map(strip_log_timestamp).collect();
    let Some(error) = lines.iter().position(|line| line.starts_with("##[error]")) else {
        return log_tail(text, budget);
    };
    let start = lines[..error]
        .iter()
        .rposition(|line| line.starts_with("##[group]"))
        .unwrap_or(0);
    log_tail(&lines[start..=error].join("\n"), budget)
}

/// GitHub prefixes each log line with its timestamp, for example `2026-10-22T20:40:37.9Z `.
fn strip_log_timestamp(line: &str) -> &str {
    let line = line.trim_start_matches('\u{feff}');
    match line.split_once(' ') {
        Some((stamp, rest)) if stamp.ends_with('Z') && stamp.contains('T') => rest,
        _ => line,
    }
}

/// The end of `text` within `budget`: the last lines that fit, the newest kept.
pub fn log_tail(text: &str, budget: LogBudget) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    let mut chars = 0;
    for line in text.lines().rev().take(budget.lines) {
        // The line and its separator count against the budget.
        let cost = line.chars().count() + 1;
        if chars + cost > budget.chars {
            // Keep the end of a long last line rather than nothing at all.
            if kept.is_empty() {
                let tail: String = line.chars().rev().take(budget.chars).collect();
                kept.push(tail.chars().rev().collect());
            }
            break;
        }
        chars += cost;
        kept.push(line.to_string());
    }
    kept.reverse();
    kept
}

/// What a CI item says about its commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiState {
    Pending,
    Success,
    Failure,
    /// Waits for a person to act (a GitLab `manual` job, a GitHub `action_required`
    /// conclusion). Waiting does not end it, so the gate ends the repo as `ci_blocked`.
    Blocked,
}

pub trait Host: Send + Sync {
    /// Routes this client's rate-limit events to the run's event stream.
    fn attach_events(&self, bus: &EventBus);
    fn probe_read_access(&self, repo: &Repo, token: &Secret) -> Result<Access, HostError>;
    fn default_branch(&self, repo: &Repo, token: &Secret) -> Result<String, HostError>;
    /// The open change request whose source is `branch`, if any. Closed or merged
    /// ones are not returned.
    fn find_change_request(
        &self,
        repo: &Repo,
        token: &Secret,
        branch: &str,
    ) -> Result<Option<ChangeRequest>, HostError>;
    /// The newest change request whose source is `branch`, in any state. Used to tell a
    /// finished change from a missing one, for example after a merge response was lost.
    fn latest_change_request(
        &self,
        repo: &Repo,
        token: &Secret,
        branch: &str,
    ) -> Result<Option<ChangeRequest>, HostError>;
    fn create_change_request(
        &self,
        repo: &Repo,
        token: &Secret,
        branch: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<ChangeRequest, HostError>;
    /// Deletes a branch. Only called after its change request was merged. A branch
    /// that is already gone counts as deleted.
    fn delete_branch(&self, repo: &Repo, token: &Secret, branch: &str) -> Result<(), HostError>;
    /// Every CI item for exactly this commit. The commit is the key, never a branch or
    /// a pull request: a later push to the same branch has its own items. Implementations
    /// must reject items that belong to another commit.
    fn commit_ci(&self, repo: &Repo, token: &Secret, sha: &str) -> Result<Vec<CiItem>, HostError>;
    /// The failed jobs of this commit's CI, first failure first, with the tail of their logs.
    /// Nested pipelines (GitLab child pipelines, GitHub reusable workflows) are included.
    /// Skipped jobs and jobs that were allowed to fail are left out.
    fn ci_failures(
        &self,
        repo: &Repo,
        token: &Secret,
        sha: &str,
        budget: LogBudget,
    ) -> Result<Vec<CiFailure>, HostError>;
    /// Starts CI on `branch` because the host started none for the pushed commit. GitHub
    /// needs the workflow file to dispatch; GitLab starts a pipeline for the ref.
    fn dispatch_ci(
        &self,
        repo: &Repo,
        token: &Secret,
        branch: &str,
        workflow: Option<&str>,
    ) -> Result<(), HostError>;
    /// Merges only if the head is still `expected_sha`; the host enforces that.
    fn merge(
        &self,
        repo: &Repo,
        token: &Secret,
        change: &ChangeRequest,
        expected_sha: &str,
    ) -> Result<MergeOutcome, HostError>;
}

/// The REST API base for a host. github.com and gitlab.com have fixed endpoints;
/// other hosts use the paths their servers serve.
pub fn api_base(host_type: HostType, host: &str) -> String {
    match (host_type, host) {
        (HostType::Github, "github.com") => "https://api.github.com".to_string(),
        (HostType::Github, _) => format!("https://{host}/api/v3"),
        (HostType::Gitlab, _) => format!("https://{host}/api/v4"),
    }
}

/// The client for a host. `base` overrides the API base (tests use a local server).
pub fn client_for(
    host_type: HostType,
    host: &str,
    base: Option<&str>,
) -> Result<Box<dyn Host>, HostError> {
    client_for_policy(host_type, host, base, DEFAULT_MAX_RATE_WAIT)
}

/// Like `client_for`, with the longest rate-limit wait from the recipe's `[rate_limit]`.
pub fn client_for_policy(
    host_type: HostType,
    host: &str,
    base: Option<&str>,
    max_rate_wait: Duration,
) -> Result<Box<dyn Host>, HostError> {
    let base = base
        .map(str::to_string)
        .unwrap_or_else(|| api_base(host_type, host));
    let http = Http::with_policy(base, host.to_string(), max_rate_wait)?;
    Ok(match host_type {
        HostType::Github => Box::new(github::GitHub::new(http)),
        HostType::Gitlab => Box::new(gitlab::GitLab::new(http)),
    })
}

/// The longest rate-limit wait fleetify takes on its own, when the recipe's `[rate_limit]`
/// does not say otherwise. A longer wait ends the repo as `rate_limited`.
pub const DEFAULT_MAX_RATE_WAIT: Duration = Duration::from_secs(15 * 60);
/// The wait when the host gives neither `Retry-After` nor a reset time.
const FALLBACK_RATE_WAIT: Duration = Duration::from_secs(60);
/// Rate-limit waits one request may take before it gives up.
const MAX_RATE_WAITS: u32 = 3;
/// Tries of one read outside the CI poll while the host fails transiently.
const READ_ATTEMPTS: u32 = 3;
/// The first wait between two tries of a read; it doubles for each further try.
const READ_RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// Runs a read again while the host fails transiently (no answer, 5xx), with back-off, and
/// returns the last error when the tries run out. Only reads go through here: a write that
/// timed out may have happened, so repeating it could open a second change request.
pub fn retry_read<T>(mut read: impl FnMut() -> Result<T, HostError>) -> Result<T, HostError> {
    let mut attempt = 1;
    loop {
        match read() {
            Err(err) if err.is_transient() && attempt < READ_ATTEMPTS => {
                thread::sleep(crate::ci::backoff(READ_RETRY_INTERVAL, attempt));
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// The text of a request error with its causes. reqwest's own text only says that the request
/// failed; the refused connect, DNS or TLS failure sits below it.
fn transport_message(err: &reqwest::Error) -> String {
    let mut message = err.to_string();
    let mut cause = std::error::Error::source(err);
    while let Some(source) = cause {
        message.push_str(": ");
        message.push_str(&source.to_string());
        cause = source.source();
    }
    message
}

/// A shared HTTP client with one API base. All repos of one host share one client, so a
/// rate limit that one repo hits also holds back the others on that host.
pub struct Http {
    client: Client,
    base: String,
    host: String,
    max_wait: Duration,
    /// Until when the host asked us to hold back. Shared by every request of this client.
    blocked_until: Mutex<Option<Instant>>,
    events: OnceLock<WeakEventBus>,
}

/// A status and body. Bodies are read as text; JSON is parsed by the caller.
pub struct Reply {
    pub status: u16,
    pub body: String,
}

/// The answer to one request, before any rate-limit handling.
enum Attempt {
    Done(Reply),
    /// The host refused for now. `wait` is how long it asked us to hold back.
    Limited {
        wait: Duration,
        reason: String,
    },
}

impl Http {
    pub fn new(base: String) -> Result<Self, HostError> {
        Self::with_policy(base.clone(), base, DEFAULT_MAX_RATE_WAIT)
    }

    /// `host` names the host in events and errors; `max_wait` is the longest rate-limit wait.
    pub fn with_policy(base: String, host: String, max_wait: Duration) -> Result<Self, HostError> {
        let client = Client::builder()
            .user_agent(concat!("fleetify/", env!("CARGO_PKG_VERSION")))
            .timeout(API_TIMEOUT)
            .build()
            .map_err(|err| HostError::Transport(transport_message(&err)))?;
        Ok(Http {
            client,
            base,
            host,
            max_wait,
            blocked_until: Mutex::new(None),
            events: OnceLock::new(),
        })
    }

    /// Routes rate-limit events of this client to the run's event stream. Set once per run.
    /// Only a weak handle is kept, so the client does not hold the stream open.
    pub fn attach_events(&self, bus: &EventBus) {
        let _ = self.events.set(bus.downgrade());
    }

    /// Sends one request. `path` starts with `/`. Headers carry the token.
    ///
    /// A rate limit within `max_wait` is waited out and the request repeated. A longer one
    /// is returned as `HostError::RateLimited` without waiting. A reply that comes with an
    /// exhausted quota stands, and the host is held back for the next requests.
    pub fn send(
        &self,
        method: Method,
        path: &str,
        headers: &[(&str, String)],
        query: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<Reply, HostError> {
        let mut waits = 0;
        loop {
            self.respect_block()?;
            match self.attempt(method.clone(), path, headers, query, body)? {
                Attempt::Done(reply) => return Ok(reply),
                Attempt::Limited { wait, reason } => {
                    if wait > self.max_wait || waits == MAX_RATE_WAITS {
                        return Err(HostError::RateLimited { wait, reason });
                    }
                    waits += 1;
                    self.hold_back(wait, &reason);
                }
            }
        }
    }

    fn attempt(
        &self,
        method: Method,
        path: &str,
        headers: &[(&str, String)],
        query: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<Attempt, HostError> {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .query(query);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .map_err(|err| HostError::Transport(transport_message(&err)))?;
        let status = response.status().as_u16();
        let response_headers = response.headers().clone();
        let body = response
            .text()
            .map_err(|err| HostError::Transport(transport_message(&err)))?;
        let now = unix_now();
        if let Some(limit) = rate_limit(status, &response_headers, &body, now) {
            return Ok(limit);
        }
        if quota_used_up(&response_headers) {
            // The reply stands; the next requests wait for the reset.
            let wait = reset_wait(&response_headers, now).unwrap_or(FALLBACK_RATE_WAIT);
            self.hold_back(wait, "the host's rate-limit quota is used up");
        }
        Ok(Attempt::Done(Reply { status, body }))
    }

    /// Waits while the host has asked us to hold back. A wait longer than the ceiling is
    /// not taken: the request fails with `RateLimited`.
    fn respect_block(&self) -> Result<(), HostError> {
        let until = *self
            .blocked_until
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(until) = until else {
            return Ok(());
        };
        let Some(wait) = until.checked_duration_since(Instant::now()) else {
            return Ok(());
        };
        if wait > self.max_wait {
            return Err(HostError::RateLimited {
                wait,
                reason: "the host asked us to hold back".to_string(),
            });
        }
        thread::sleep(wait);
        Ok(())
    }

    /// Records that the host asked us to hold back for `wait`, and tells the event stream.
    /// Waits above the ceiling are recorded but not reported as taken.
    fn hold_back(&self, wait: Duration, reason: &str) {
        {
            let mut blocked = self
                .blocked_until
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let until = Instant::now() + wait;
            if blocked.is_none_or(|current| current < until) {
                *blocked = Some(until);
            }
        }
        if wait <= self.max_wait
            && let Some(bus) = self.events.get()
        {
            bus.emit(Event::RateLimited {
                host: self.host.clone(),
                wait_ms: wait.as_millis() as u64,
                reason: reason.to_string(),
            });
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

/// The wait the reset header of the quota asks for (GitHub `x-ratelimit-reset`, GitLab
/// `RateLimit-Reset`), counted from `now`.
fn reset_wait(headers: &HeaderMap, now: u64) -> Option<Duration> {
    let reset = header_u64(headers, "x-ratelimit-reset")
        .or_else(|| header_u64(headers, "ratelimit-reset"))?;
    Some(Duration::from_secs(reset.saturating_sub(now)))
}

/// The wait `Retry-After` asks for. It is either seconds or an HTTP date (IMF-fixdate, RFC
/// 9110), counted from `now`.
fn retry_after_wait(headers: &HeaderMap, now: u64) -> Option<Duration> {
    let value = headers.get("retry-after")?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = imf_fixdate_epoch(value)?;
    Some(Duration::from_secs(date.saturating_sub(now)))
}

/// Epoch seconds of an IMF-fixdate such as `Wed, 21 Oct 2015 07:28:00 GMT`.
fn imf_fixdate_epoch(text: &str) -> Option<u64> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let [_, day, month, year, time, zone] = parts.as_slice() else {
        return None;
    };
    if *zone != "GMT" {
        return None;
    }
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = MONTHS.iter().position(|name| name == month)? as i64 + 1;
    let day: i64 = day.parse().ok()?;
    let year: i64 = year.parse().ok()?;
    let mut clock = time.split(':').map(|part| part.parse::<i64>().ok());
    let (hour, minute, second) = (clock.next()??, clock.next()??, clock.next()??);
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(seconds).ok()
}

/// Days since 1970-01-01 of a Gregorian date (the civil-from-days algorithm, inverted).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The quota is used up when the remaining count says zero.
fn quota_used_up(headers: &HeaderMap) -> bool {
    let remaining = headers
        .get("x-ratelimit-remaining")
        .or_else(|| headers.get("ratelimit-remaining"))
        .and_then(|value| value.to_str().ok());
    remaining.is_some_and(|value| value.trim() == "0")
}

/// Recognises a refusal for rate-limit reasons, per the hosts' docs:
/// - HTTP 429 is always a rate limit (GitHub, GitLab).
/// - HTTP 403 with `x-ratelimit-remaining: 0` is GitHub's primary limit.
/// - HTTP 403 with `retry-after` or a "rate limit" message is GitHub's secondary limit.
///   The message text is not documented, so the body check is a heuristic.
///
/// `Retry-After` in seconds wins over the reset time.
fn rate_limit(status: u16, headers: &HeaderMap, body: &str, now: u64) -> Option<Attempt> {
    let reason = match status {
        429 => "HTTP 429 Too Many Requests".to_string(),
        403 if quota_used_up(headers) => "primary rate limit (HTTP 403)".to_string(),
        403 if headers.contains_key("retry-after")
            || body.to_ascii_lowercase().contains("rate limit") =>
        {
            "secondary rate limit (HTTP 403)".to_string()
        }
        _ => return None,
    };
    let wait = retry_after_wait(headers, now)
        .or_else(|| reset_wait(headers, now))
        .unwrap_or(FALLBACK_RATE_WAIT);
    Some(Attempt::Limited { wait, reason })
}

/// Maps the status of a read to an access result. `None` for other statuses.
pub fn access_from_status(status: u16) -> Option<Access> {
    match status {
        200 => Some(Access::Ok),
        401 => Some(Access::Unauthorized),
        403 => Some(Access::Forbidden),
        404 => Some(Access::NotFound),
        _ => None,
    }
}

/// Turns an unexpected reply into an error. The message is the host's `message`
/// field when present, otherwise the start of the body.
pub fn unexpected(reply: &Reply) -> HostError {
    let message = serde_json::from_str::<Value>(&reply.body)
        .ok()
        .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| reply.body.chars().take(MAX_BODY_CHARS).collect());
    HostError::Status {
        status: reply.status,
        message,
    }
}

/// Parses a JSON reply body.
pub fn parse_json(reply: &Reply) -> Result<Value, HostError> {
    serde_json::from_str(&reply.body).map_err(|err| HostError::Parse(err.to_string()))
}

/// The message of a refusal (a 405, 406 or 409 from a merge), used as the block reason.
pub fn refusal_reason(reply: &Reply) -> String {
    match unexpected(reply) {
        HostError::Status { status, message } => format!("HTTP {status}: {message}"),
        other => other.to_string(),
    }
}

/// Classification of a TLS-level failure, for the startup self-test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsCheck {
    Ok,
    /// The certificate chain is not trusted: the internal CA is missing from the OS store.
    UntrustedCertificate(String),
    /// Anything else (no route, timeout, DNS). Reported, not fatal.
    Unreachable(String),
}

/// One HTTPS request to the API base, classifying failures. Used at startup, once
/// per host. Certificate errors stop the run; other errors are only reported.
pub fn tls_self_test(base: &str) -> TlsCheck {
    let client = match Client::builder().timeout(Duration::from_secs(15)).build() {
        Ok(client) => client,
        Err(err) => return TlsCheck::Unreachable(err.to_string()),
    };
    match client.get(base).send() {
        Ok(_) => TlsCheck::Ok,
        Err(err) => classify_tls_error(&format!("{err:?}")),
    }
}

/// Looks for certificate-trust failures in an error's debug text.
pub fn classify_tls_error(text: &str) -> TlsCheck {
    let lower = text.to_ascii_lowercase();
    if lower.contains("certificate")
        || lower.contains("unknownissuer")
        || lower.contains("invalid peer")
    {
        TlsCheck::UntrustedCertificate(text.chars().take(MAX_BODY_CHARS).collect())
    } else {
        TlsCheck::Unreachable(text.chars().take(MAX_BODY_CHARS).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_tail_keeps_the_end_within_both_limits() {
        let text = (1..=10)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let by_lines = log_tail(
            &text,
            LogBudget {
                lines: 3,
                chars: 1000,
            },
        );
        assert_eq!(by_lines, vec!["line 8", "line 9", "line 10"]);
        let by_chars = log_tail(
            &text,
            LogBudget {
                lines: 100,
                chars: 15,
            },
        );
        assert_eq!(by_chars, vec!["line 9", "line 10"]);
    }

    #[test]
    fn log_tail_keeps_the_end_of_one_very_long_line() {
        let long = format!("{}ERROR at the end", "x".repeat(500));
        let tail = log_tail(
            &long,
            LogBudget {
                lines: 100,
                chars: 20,
            },
        );
        assert_eq!(tail.len(), 1);
        assert!(tail[0].ends_with("ERROR at the end"), "{tail:?}");
        assert!(tail[0].chars().count() <= 20);
    }

    #[test]
    fn log_tail_of_an_empty_log_is_empty() {
        assert!(
            log_tail(
                "",
                LogBudget {
                    lines: 100,
                    chars: 1000
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn default_api_bases() {
        assert_eq!(
            api_base(HostType::Github, "github.com"),
            "https://api.github.com"
        );
        assert_eq!(
            api_base(HostType::Github, "ghe.example.com"),
            "https://ghe.example.com/api/v3"
        );
        assert_eq!(
            api_base(HostType::Gitlab, "gitlab.com"),
            "https://gitlab.com/api/v4"
        );
        assert_eq!(
            api_base(HostType::Gitlab, "gitlab.example.com"),
            "https://gitlab.example.com/api/v4"
        );
    }

    #[test]
    fn only_no_answer_429_and_5xx_are_transient() {
        let status = |status| HostError::Status {
            status,
            message: String::new(),
        };
        assert!(HostError::Transport("timeout".into()).is_transient());
        assert!(status(429).is_transient());
        assert!(status(502).is_transient());
        assert!(!status(401).is_transient());
        assert!(!status(404).is_transient());
        assert!(!HostError::Parse("bad".into()).is_transient());
        assert!(!HostError::Unsupported("x".into()).is_transient());
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, value.parse().unwrap());
        }
        map
    }

    #[test]
    fn retry_after_wins_over_the_reset_time() {
        let h = headers(&[("retry-after", "7"), ("x-ratelimit-reset", "2000")]);
        assert_eq!(
            header_u64(&h, "retry-after").map(Duration::from_secs),
            Some(Duration::from_secs(7))
        );
        match rate_limit(429, &h, "", 1000) {
            Some(Attempt::Limited { wait, .. }) => assert_eq!(wait, Duration::from_secs(7)),
            _ => panic!("429 is a rate limit"),
        }
    }

    #[test]
    fn a_read_is_retried_only_while_the_host_fails_transiently() {
        let mut calls = 0;
        let answer = retry_read(|| {
            calls += 1;
            if calls == 1 {
                Err(HostError::Transport("no answer".into()))
            } else {
                Ok("main")
            }
        });
        assert_eq!(answer.unwrap(), "main");
        assert_eq!(calls, 2);

        let mut calls = 0;
        let refused: Result<(), HostError> = retry_read(|| {
            calls += 1;
            Err(HostError::Status {
                status: 404,
                message: "gone".into(),
            })
        });
        assert!(refused.is_err());
        assert_eq!(calls, 1, "a permanent error is not repeated");
    }

    #[test]
    fn retry_after_takes_seconds_or_an_http_date() {
        let now = 1_445_412_480 - 90;
        let date = headers(&[("retry-after", "Wed, 21 Oct 2015 07:28:00 GMT")]);
        assert_eq!(retry_after_wait(&date, now), Some(Duration::from_secs(90)));
        let seconds = headers(&[("retry-after", "7")]);
        assert_eq!(
            retry_after_wait(&seconds, now),
            Some(Duration::from_secs(7))
        );
        let past = headers(&[("retry-after", "Wed, 21 Oct 2015 07:28:00 GMT")]);
        assert_eq!(retry_after_wait(&past, now + 500), Some(Duration::ZERO));
        let garbage = headers(&[("retry-after", "soon, maybe")]);
        assert_eq!(retry_after_wait(&garbage, now), None);
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    }

    #[test]
    fn a_reset_time_is_counted_from_now() {
        let h = headers(&[("x-ratelimit-reset", "1090")]);
        assert_eq!(reset_wait(&h, 1000), Some(Duration::from_secs(90)));
        // Already in the past: no wait, not a negative one.
        assert_eq!(reset_wait(&h, 2000), Some(Duration::ZERO));
    }

    #[test]
    fn gitlab_reset_header_is_read_too() {
        let h = headers(&[("ratelimit-remaining", "0"), ("ratelimit-reset", "1060")]);
        assert!(quota_used_up(&h));
        assert_eq!(reset_wait(&h, 1000), Some(Duration::from_secs(60)));
    }

    #[test]
    fn a_403_is_a_rate_limit_only_with_an_exhausted_quota_or_a_secondary_limit() {
        let none = HeaderMap::new();
        assert!(rate_limit(403, &none, r#"{"message":"Forbidden"}"#, 0).is_none());
        let exhausted = headers(&[("x-ratelimit-remaining", "0")]);
        assert!(rate_limit(403, &exhausted, "{}", 0).is_some());
        let retry = headers(&[("retry-after", "30")]);
        assert!(rate_limit(403, &retry, "{}", 0).is_some());
        assert!(rate_limit(403, &none, "You have exceeded a secondary rate limit.", 0).is_some());
        assert!(rate_limit(401, &none, "rate limit", 0).is_none());
    }

    #[test]
    fn a_rate_limit_is_neither_transient_nor_a_permanent_error() {
        let err = HostError::RateLimited {
            wait: Duration::from_secs(3600),
            reason: "HTTP 429".into(),
        };
        assert!(err.is_rate_limited());
        assert!(!err.is_transient());
    }

    #[test]
    fn statuses_map_to_access() {
        assert_eq!(access_from_status(200), Some(Access::Ok));
        assert_eq!(access_from_status(401), Some(Access::Unauthorized));
        assert_eq!(access_from_status(403), Some(Access::Forbidden));
        assert_eq!(access_from_status(404), Some(Access::NotFound));
        assert_eq!(access_from_status(500), None);
    }

    #[test]
    fn unexpected_prefers_the_host_message() {
        let reply = Reply {
            status: 422,
            body: r#"{"message":"Validation Failed"}"#.into(),
        };
        assert_eq!(
            unexpected(&reply),
            HostError::Status {
                status: 422,
                message: "Validation Failed".into()
            }
        );
    }

    #[test]
    fn certificate_failures_are_classified() {
        assert!(matches!(
            classify_tls_error(
                "reqwest::Error { kind: Request, source: hyper_util::Error(Connect, Custom { kind: Other, error: InvalidCertificate(UnknownIssuer) }) }"
            ),
            TlsCheck::UntrustedCertificate(_)
        ));
        assert!(matches!(
            classify_tls_error("error trying to connect: dns error: no such host"),
            TlsCheck::Unreachable(_)
        ));
    }

    #[test]
    fn failed_step_log_ends_at_the_error_and_skips_the_post_steps() {
        let log = "\
2026-10-22T20:40:30.0000000Z ##[group]Run dtolnay/rust-toolchain@stable
2026-10-22T20:40:31.0000000Z installed
2026-10-22T20:40:32.0000000Z ##[endgroup]
2026-10-22T20:40:33.0000000Z ##[group]Run cargo fmt --all -- --check
2026-10-22T20:40:34.0000000Z Diff in src/config.rs:480:
2026-10-22T20:40:35.0000000Z +        assert_eq!(
2026-10-22T20:40:36.0000000Z ##[error]Process completed with exit code 1.
2026-10-22T20:40:37.0000000Z ##[endgroup]
2026-10-22T20:40:38.0000000Z ##[group]Post Run actions/checkout@v4
2026-10-22T20:40:39.0000000Z git config --unset-all http.extraheader
2026-10-22T20:40:40.0000000Z ##[endgroup]";
        let tail = failed_step_log(
            log,
            LogBudget {
                lines: 100,
                chars: 1000,
            },
        );
        assert_eq!(
            tail.first().map(String::as_str),
            Some("##[group]Run cargo fmt --all -- --check")
        );
        assert_eq!(
            tail.last().map(String::as_str),
            Some("##[error]Process completed with exit code 1.")
        );
        assert!(
            !tail.iter().any(|line| line.contains("extraheader")),
            "{tail:?}"
        );
    }

    #[test]
    fn failed_step_log_without_an_error_line_is_the_plain_tail() {
        let tail = failed_step_log(
            "one\ntwo\nthree",
            LogBudget {
                lines: 2,
                chars: 1000,
            },
        );
        assert_eq!(tail, vec!["two", "three"]);
    }
}
