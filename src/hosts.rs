//! Host layer: GitHub and GitLab over their REST APIs (PLAN.md, "Host layer").
//! Everything host-specific sits behind [`Host`]. Tokens are passed per call and
//! sent only in request headers, never in URLs. CI is not part of this trait.

pub mod github;
pub mod gitlab;

use std::fmt;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::Method;
use reqwest::blocking::Client;
use reqwest::header::HeaderMap;
use serde_json::Value;

use crate::config::HostType;
use crate::credentials::Secret;
use crate::events::{Event, EventBus, WeakEventBus};
use crate::repos::Repo;

/// Timeout for one API call.
const API_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest error body kept in an error message.
const MAX_BODY_CHARS: usize = 300;

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

/// What a CI item says about its commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiState {
    Pending,
    Success,
    Failure,
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

/// Like `client_for`, with the longest rate-limit wait from `[rate_limit]` in config.toml.
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

/// The longest rate-limit wait fleetify takes on its own, when `[rate_limit]` in config.toml
/// does not say otherwise. A longer wait ends the repo as `rate_limited`.
pub const DEFAULT_MAX_RATE_WAIT: Duration = Duration::from_secs(15 * 60);
/// The wait when the host gives neither `Retry-After` nor a reset time.
const FALLBACK_RATE_WAIT: Duration = Duration::from_secs(60);
/// Rate-limit waits one request may take before it gives up.
const MAX_RATE_WAITS: u32 = 3;

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
            .map_err(|err| HostError::Transport(err.to_string()))?;
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
            .map_err(|err| HostError::Transport(err.to_string()))?;
        let status = response.status().as_u16();
        let response_headers = response.headers().clone();
        let body = response
            .text()
            .map_err(|err| HostError::Transport(err.to_string()))?;
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
        403 if header_u64(headers, "retry-after").is_some()
            || body.to_ascii_lowercase().contains("rate limit") =>
        {
            "secondary rate limit (HTTP 403)".to_string()
        }
        _ => return None,
    };
    let wait = header_u64(headers, "retry-after")
        .map(Duration::from_secs)
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
}
