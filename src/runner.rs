//! Command runner: argv model, placeholder substitution, streaming output and
//! timeouts that terminate the whole process tree. Every external process
//! (git, verifier, agent, prepare script) goes through here.
//!
//! No implicit shell: a command is an argument list. Placeholders are
//! substituted per element, never into a joined string.

use std::error::Error;
use std::fmt;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use command_group::CommandGroup;

use crate::events::{Event, EventBus, Outcome, Redactor, Stream};

/// Variables passed through by [`EnvPolicy::minimal`]. Everything else is dropped.
pub const BASE_ENV: &[&str] = &[
    "PATH",
    "PATHEXT",
    "HOME",
    "USER",
    "USERNAME",
    "USERPROFILE",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TMPDIR",
    "TMP",
    "TEMP",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
];

/// Output lines kept in memory per command. Every line is still emitted as an event.
const MAX_RETAINED_LINES: usize = 10_000;
const POLL: Duration = Duration::from_millis(50);
/// How long to wait for output pipes to close after the process has exited.
/// Grandchildren that escaped the process group may keep them open.
const PIPE_GRACE: Duration = Duration::from_secs(2);
/// How long to wait for a killed process tree to report its exit.
const KILL_WAIT: Duration = Duration::from_secs(5);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A command as declared in a recipe: one argument list for both OSes, or one per OS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSpec {
    Argv(Vec<String>),
    PerOs {
        posix: Option<Vec<String>>,
        windows: Option<Vec<String>>,
    },
}

/// Errors while choosing or rendering a command. Messages name the field
/// or placeholder involved; they never contain environment values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecError {
    EmptyArgv,
    MissingForOs { os: &'static str },
    UnknownPlaceholder { name: String, element: String },
}

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpecError::EmptyArgv => write!(f, "command has an empty argument list"),
            SpecError::MissingForOs { os } => write!(f, "command has no variant for {os}"),
            SpecError::UnknownPlaceholder { name, element } => {
                write!(f, "unknown placeholder {{{name}}} in argument {element:?}")
            }
        }
    }
}

impl Error for SpecError {}

impl CommandSpec {
    /// The argument list that applies to the given OS.
    pub fn argv_for(&self, windows: bool) -> Result<&[String], SpecError> {
        let (list, os) = match self {
            CommandSpec::Argv(list) => (Some(list), None),
            CommandSpec::PerOs {
                posix,
                windows: win,
            } => {
                if windows {
                    (win.as_ref(), Some("windows"))
                } else {
                    (posix.as_ref(), Some("posix"))
                }
            }
        };
        match list {
            Some(list) if !list.is_empty() => Ok(list),
            Some(_) => Err(SpecError::EmptyArgv),
            None => Err(SpecError::MissingForOs {
                os: os.unwrap_or("this platform"),
            }),
        }
    }

    /// The argument list for the given OS with placeholders substituted in each element.
    pub fn render(&self, windows: bool, vars: &[(&str, &str)]) -> Result<Vec<String>, SpecError> {
        self.argv_for(windows)?
            .iter()
            .map(|element| substitute(element, vars))
            .collect()
    }
}

/// Replaces `{name}` placeholders in one argument, in a single pass. Substituted
/// values are never scanned again. Braces that do not enclose a placeholder name
/// (for example `{}` or `{a b}`) stay literal. An unknown name is an error.
pub fn substitute(element: &str, vars: &[(&str, &str)]) -> Result<String, SpecError> {
    let mut out = String::with_capacity(element.len());
    let mut rest = element;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) if is_placeholder_name(&after[..end]) => {
                let name = &after[..end];
                let value = vars
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| *value)
                    .ok_or_else(|| SpecError::UnknownPlaceholder {
                        name: name.to_string(),
                        element: element.to_string(),
                    })?;
                out.push_str(value);
                rest = &after[end + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}

fn is_placeholder_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// Which environment a child process gets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum EnvPolicy {
    /// Inherit the full environment of the orchestrator.
    #[default]
    Inherit,
    /// Start from an empty environment. `names` are copied from the parent when set;
    /// `set` adds fixed values.
    Allowlist {
        names: Vec<String>,
        set: Vec<(String, String)>,
    },
}

impl EnvPolicy {
    /// Allowlist with only the [`BASE_ENV`] variables.
    pub fn minimal() -> Self {
        EnvPolicy::Allowlist {
            names: BASE_ENV.iter().map(|s| s.to_string()).collect(),
            set: Vec::new(),
        }
    }
}

/// One process to run.
#[derive(Debug, Clone)]
pub struct RunRequest {
    /// Name shown in events and output prefixes, usually the repo.
    pub label: String,
    /// Program and arguments, already rendered.
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub timeout: Option<Duration>,
    pub env: EnvPolicy,
}

impl RunRequest {
    pub fn new(label: impl Into<String>, argv: Vec<String>) -> Self {
        RunRequest {
            label: label.into(),
            argv,
            cwd: None,
            timeout: None,
            env: EnvPolicy::Inherit,
        }
    }
}

/// One line of child output, already redacted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputLine {
    pub stream: Stream,
    pub text: String,
}

/// Result of one run. Output is redacted and capped at [`MAX_RETAINED_LINES`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub id: u64,
    pub outcome: Outcome,
    pub duration: Duration,
    pub lines: Vec<OutputLine>,
    /// Lines that were emitted as events but not kept in `lines`.
    pub lines_dropped: usize,
}

impl CommandResult {
    pub fn is_success(&self) -> bool {
        self.outcome.is_success()
    }

    /// All kept output lines, joined with newlines.
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

enum Msg {
    Line(Stream, String),
    Closed,
}

/// Runs processes, emitting events on the bus and returning their results.
#[derive(Clone)]
pub struct Runner {
    bus: EventBus,
    redactor: Redactor,
}

impl Runner {
    pub fn new(bus: EventBus, redactor: Redactor) -> Self {
        Runner { bus, redactor }
    }

    pub fn bus(&self) -> &EventBus {
        &self.bus
    }

    /// Runs the request to completion, or until its timeout expires.
    pub fn run(&self, req: RunRequest) -> CommandResult {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let started = Instant::now();
        self.bus.emit(Event::CommandStarted {
            command: id,
            label: req.label.clone(),
            argv: req.argv.iter().map(|a| self.redactor.redact(a)).collect(),
            cwd: req
                .cwd
                .as_ref()
                .map(|p| self.redactor.redact(&p.display().to_string())),
            timeout_s: req.timeout.map(|t| t.as_secs()),
        });

        let mut lines = Vec::new();
        let mut dropped = 0;
        let outcome = self.supervise(id, &req, &mut lines, &mut dropped);
        let duration = started.elapsed();

        self.bus.emit(Event::CommandFinished {
            command: id,
            label: req.label.clone(),
            outcome: outcome.clone(),
            duration_ms: duration.as_millis().try_into().unwrap_or(u64::MAX),
        });
        CommandResult {
            id,
            outcome,
            duration,
            lines,
            lines_dropped: dropped,
        }
    }

    fn supervise(
        &self,
        id: u64,
        req: &RunRequest,
        lines: &mut Vec<OutputLine>,
        dropped: &mut usize,
    ) -> Outcome {
        let Some((program, args)) = req.argv.split_first() else {
            return Outcome::SpawnFailed {
                message: SpecError::EmptyArgv.to_string(),
            };
        };
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &req.cwd {
            command.current_dir(cwd);
        }
        apply_env(&mut command, &req.env);

        let mut child = match command.group_spawn() {
            Ok(child) => child,
            Err(err) => {
                return Outcome::SpawnFailed {
                    message: self
                        .redactor
                        .redact(&format!("cannot start {program}: {err}")),
                };
            }
        };

        let started = Instant::now();
        let (tx, rx) = mpsc::channel::<Msg>();
        let mut open_pipes = 0;
        if let Some(pipe) = child.inner().stdout.take() {
            spawn_reader(pipe, Stream::Stdout, tx.clone());
            open_pipes += 1;
        }
        if let Some(pipe) = child.inner().stderr.take() {
            spawn_reader(pipe, Stream::Stderr, tx.clone());
            open_pipes += 1;
        }
        drop(tx);

        let deadline = req.timeout.map(|t| started + t);
        let mut status = None;
        let mut exited_at = None;
        let mut killed_at = None;

        loop {
            match rx.recv_timeout(POLL) {
                Ok(Msg::Line(stream, text)) => {
                    self.record(id, &req.label, stream, text, lines, dropped)
                }
                Ok(Msg::Closed) => open_pipes -= 1,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    open_pipes = 0;
                    thread::sleep(POLL);
                }
            }

            if status.is_none() {
                match child.try_wait() {
                    Ok(Some(exit)) => {
                        status = Some(exit);
                        exited_at = Some(Instant::now());
                    }
                    Ok(None) => {}
                    Err(err) => {
                        let _ = child.kill();
                        return Outcome::Error {
                            message: self
                                .redactor
                                .redact(&format!("cannot wait for process: {err}")),
                        };
                    }
                }
            }

            if status.is_none()
                && killed_at.is_none()
                && let Some(deadline) = deadline
                && Instant::now() >= deadline
            {
                let _ = child.kill();
                killed_at = Some(Instant::now());
            }

            if let Some(killed) = killed_at
                && (status.is_some() || killed.elapsed() >= KILL_WAIT)
            {
                break;
            }
            if let Some(exited) = exited_at
                && (open_pipes == 0 || exited.elapsed() >= PIPE_GRACE)
            {
                break;
            }
        }

        if killed_at.is_some() {
            return Outcome::TimedOut;
        }
        if exited_at.is_some() && open_pipes > 0 {
            // Something escaped the group but still holds our pipes: clean up.
            let _ = child.kill();
        }
        match status.map(|s| s.code()) {
            Some(code) => Outcome::Exited { code },
            None => Outcome::Error {
                message: "process ended without an exit status".to_string(),
            },
        }
    }

    fn record(
        &self,
        id: u64,
        label: &str,
        stream: Stream,
        text: String,
        lines: &mut Vec<OutputLine>,
        dropped: &mut usize,
    ) {
        let text = self.redactor.redact(&text);
        self.bus.emit(Event::OutputLine {
            command: id,
            label: label.to_string(),
            stream,
            line: text.clone(),
        });
        if lines.len() < MAX_RETAINED_LINES {
            lines.push(OutputLine { stream, text });
        } else {
            *dropped += 1;
        }
    }
}

fn apply_env(command: &mut Command, policy: &EnvPolicy) {
    if let EnvPolicy::Allowlist { names, set } = policy {
        command.env_clear();
        for name in names {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        for (key, value) in set {
            command.env(key, value);
        }
    }
}

fn spawn_reader<R: Read + Send + 'static>(pipe: R, stream: Stream, tx: mpsc::Sender<Msg>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    while matches!(buf.last(), Some(b'\n' | b'\r')) {
                        buf.pop();
                    }
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    if tx.send(Msg::Line(stream, text)).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = tx.send(Msg::Closed);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn placeholders_are_replaced_per_element() {
        let out = substitute("{repo}/x", &[("repo", "a b")]).unwrap();
        assert_eq!(out, "a b/x");
    }

    #[test]
    fn substituted_values_are_not_scanned_again() {
        let out = substitute("{repo}", &[("repo", "{tools}"), ("tools", "/nope")]).unwrap();
        assert_eq!(out, "{tools}");
    }

    #[test]
    fn braces_that_are_not_placeholders_stay_literal() {
        assert_eq!(substitute("{}", &[]).unwrap(), "{}");
        assert_eq!(substitute("{a b}", &[]).unwrap(), "{a b}");
        assert_eq!(substitute("x{", &[]).unwrap(), "x{");
    }

    #[test]
    fn unknown_placeholder_is_an_error_naming_it() {
        let err = substitute("{reop}", &[("repo", "x")]).unwrap_err();
        assert_eq!(
            err,
            SpecError::UnknownPlaceholder {
                name: "reop".into(),
                element: "{reop}".into()
            }
        );
    }

    #[test]
    fn argv_applies_to_both_oses() {
        let spec = CommandSpec::Argv(argv(&["git", "status"]));
        assert_eq!(spec.argv_for(true).unwrap(), &argv(&["git", "status"])[..]);
        assert_eq!(spec.argv_for(false).unwrap(), &argv(&["git", "status"])[..]);
    }

    #[test]
    fn per_os_variant_is_selected_and_missing_one_is_an_error() {
        let spec = CommandSpec::PerOs {
            posix: Some(argv(&["bash", "x.sh"])),
            windows: None,
        };
        assert_eq!(spec.argv_for(false).unwrap(), &argv(&["bash", "x.sh"])[..]);
        assert_eq!(
            spec.argv_for(true).unwrap_err(),
            SpecError::MissingForOs { os: "windows" }
        );
    }

    #[test]
    fn empty_argv_is_rejected() {
        let spec = CommandSpec::Argv(vec![]);
        assert_eq!(spec.argv_for(false).unwrap_err(), SpecError::EmptyArgv);
    }

    #[test]
    fn render_substitutes_every_element() {
        let spec = CommandSpec::Argv(argv(&["verify", "{repo}"]));
        let rendered = spec.render(false, &[("repo", "team/a")]).unwrap();
        assert_eq!(rendered, argv(&["verify", "team/a"]));
    }
}
