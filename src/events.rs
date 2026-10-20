//! Typed event stream. The orchestrator emits [`Event`]s into an [`EventBus`];
//! renderers, the event log and the report consume them. Nothing shown to a
//! user comes from anywhere else (PLAN.md, "Events, renderers and the report").

use std::sync::{Arc, Mutex, mpsc};

use serde::{Deserialize, Serialize};

/// Output stream a line was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    Stdout,
    Stderr,
}

/// How a command ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// The process ended on its own. `code` is `None` when it was killed by a
    /// signal (Unix).
    Exited { code: Option<i32> },
    /// The timeout expired and the process tree was terminated.
    TimedOut,
    /// The program could not be started.
    SpawnFailed { message: String },
    /// Ctrl-C arrived while the process ran; the process tree was terminated.
    Interrupted,
    /// The process started but the tool could not supervise it to the end.
    Error { message: String },
}

impl Outcome {
    /// True only for a normal exit with code 0.
    pub fn is_success(&self) -> bool {
        matches!(self, Outcome::Exited { code: Some(0) })
    }
}

/// One event of the run. Serialised as JSON lines in `events.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    CommandStarted {
        command: u64,
        label: String,
        argv: Vec<String>,
        cwd: Option<String>,
        timeout_s: Option<u64>,
    },
    OutputLine {
        command: u64,
        label: String,
        stream: Stream,
        line: String,
    },
    CommandFinished {
        command: u64,
        label: String,
        outcome: Outcome,
        duration_ms: u64,
    },
    /// A repo moved to a new phase (`cloning`, `verifying`, ...).
    RepoPhase { repo: String, phase: String },
    /// The result of one repo in a run (dry run for now, full results from M7).
    RepoFinished {
        repo: String,
        result: String,
        message: String,
    },
    /// A wave is done. `remaining` repos are still waiting for the next decision.
    WaveFinished {
        wave: usize,
        total: usize,
        successful: usize,
        remaining: usize,
    },
    /// A command printed nothing for `idle_s` seconds. Sent once per quiet stretch.
    CommandIdle {
        command: u64,
        label: String,
        idle_s: u64,
    },
    /// The grants an agent starts with, sent before the agent runs.
    PermissionsEffective {
        repo: String,
        agent: String,
        commands: Vec<String>,
        web: bool,
        directories: Vec<String>,
        unrestricted: bool,
    },
}

/// Fan-out of events to any number of subscribers. Subscribers that have been
/// dropped are removed on the next emit.
#[derive(Clone, Default)]
pub struct EventBus {
    subscribers: Arc<Mutex<Vec<mpsc::Sender<Event>>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self) -> mpsc::Receiver<Event> {
        let (tx, rx) = mpsc::channel();
        self.lock().push(tx);
        rx
    }

    pub fn emit(&self, event: Event) {
        self.lock().retain(|tx| tx.send(event.clone()).is_ok());
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<mpsc::Sender<Event>>> {
        // A poisoned lock only means a subscriber panicked; the list is still valid.
        self.subscribers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Replaces known secret values in text. Applied to every argv, output line and
/// log line before it leaves the orchestrator. Token rules come with M2.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    secrets: Vec<String>,
}

const REDACTED: &str = "***";

impl Redactor {
    /// Empty secrets are ignored. Longer secrets are replaced first, so a secret
    /// that contains another one is masked as a whole.
    pub fn new<I, S>(secrets: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut redactor = Redactor::default();
        for secret in secrets {
            redactor.add(secret);
        }
        redactor
    }

    pub fn add(&mut self, secret: impl Into<String>) {
        let secret = secret.into();
        if !secret.is_empty() && !self.secrets.contains(&secret) {
            self.secrets.push(secret);
            self.secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        }
    }

    pub fn redact(&self, text: &str) -> String {
        self.secrets.iter().fold(text.to_string(), |acc, secret| {
            acc.replace(secret.as_str(), REDACTED)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_delivers_to_every_subscriber_and_drops_closed_ones() {
        let bus = EventBus::new();
        let a = bus.subscribe();
        let b = bus.subscribe();
        drop(b);
        bus.emit(Event::CommandFinished {
            command: 1,
            label: "x".into(),
            outcome: Outcome::Exited { code: Some(0) },
            duration_ms: 3,
        });
        assert!(matches!(
            a.try_recv(),
            Ok(Event::CommandFinished { command: 1, .. })
        ));
        assert_eq!(bus.lock().len(), 1);
    }

    #[test]
    fn events_serialise_with_their_tag() {
        let event = Event::OutputLine {
            command: 7,
            label: "repo".into(),
            stream: Stream::Stderr,
            line: "boom".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"event":"output_line","command":7,"label":"repo","stream":"stderr","line":"boom"}"#
        );
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn outcome_success_only_for_code_zero() {
        assert!(Outcome::Exited { code: Some(0) }.is_success());
        assert!(!Outcome::Exited { code: Some(1) }.is_success());
        assert!(!Outcome::Exited { code: None }.is_success());
        assert!(!Outcome::TimedOut.is_success());
    }

    #[test]
    fn redactor_masks_longest_secret_first_and_ignores_empty() {
        let redactor = Redactor::new(["", "abc", "abcdef"]);
        assert_eq!(redactor.redact("x abcdef y abc z"), "x *** y *** z");
        assert_eq!(redactor.redact("nothing here"), "nothing here");
    }
}
