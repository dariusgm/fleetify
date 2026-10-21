//! Host layer: GitHub and GitLab over their REST APIs (PLAN.md, "Host layer").
//! Everything host-specific sits behind [`Host`]. Tokens are passed per call and
//! sent only in request headers, never in URLs. CI is not part of this trait.

pub mod github;
pub mod gitlab;

use std::fmt;
use std::time::Duration;

use reqwest::Method;
use reqwest::blocking::Client;
use serde_json::Value;

use crate::config::HostType;
use crate::credentials::Secret;
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
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostError::Status { status, message } => write!(f, "HTTP {status}: {message}"),
            HostError::Transport(message) => write!(f, "cannot reach the host: {message}"),
            HostError::Parse(message) => write!(f, "unexpected answer from the host: {message}"),
            HostError::Unsupported(message) => write!(f, "not supported yet: {message}"),
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
            HostError::Parse(_) | HostError::Unsupported(_) => false,
        }
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
    let base = base
        .map(str::to_string)
        .unwrap_or_else(|| api_base(host_type, host));
    let http = Http::new(base)?;
    Ok(match host_type {
        HostType::Github => Box::new(github::GitHub::new(http)),
        HostType::Gitlab => Box::new(gitlab::GitLab::new(http)),
    })
}

/// A shared HTTP client with one API base.
pub struct Http {
    client: Client,
    base: String,
}

/// A status and body. Bodies are read as text; JSON is parsed by the caller.
pub struct Reply {
    pub status: u16,
    pub body: String,
}

impl Http {
    pub fn new(base: String) -> Result<Self, HostError> {
        let client = Client::builder()
            .user_agent(concat!("fleetify/", env!("CARGO_PKG_VERSION")))
            .timeout(API_TIMEOUT)
            .build()
            .map_err(|err| HostError::Transport(err.to_string()))?;
        Ok(Http { client, base })
    }

    /// Sends one request. `path` starts with `/`. Headers carry the token.
    pub fn send(
        &self,
        method: Method,
        path: &str,
        headers: &[(&str, String)],
        query: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<Reply, HostError> {
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
        let body = response
            .text()
            .map_err(|err| HostError::Transport(err.to_string()))?;
        Ok(Reply { status, body })
    }
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
