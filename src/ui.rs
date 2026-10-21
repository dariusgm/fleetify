//! Renderers over the event stream. The plain renderer (M1) prints one prefixed
//! line per event. The live terminal view is a later milestone (M13).

use std::sync::mpsc::Receiver;

use crate::events::{Event, Outcome};

/// One line of plain output for an event, in the style of `docker compose logs`.
pub fn plain_line(event: &Event) -> String {
    match event {
        Event::CommandStarted {
            label, argv, cwd, ..
        } => match cwd {
            Some(cwd) => format!("{label} | $ {} (in {cwd})", argv.join(" ")),
            None => format!("{label} | $ {}", argv.join(" ")),
        },
        Event::OutputLine { label, line, .. } => format!("{label} | {line}"),
        Event::RepoPhase { repo, phase } => format!("{repo} | phase {phase}"),
        Event::RepoFinished {
            repo,
            result,
            message,
        } => format!("{repo} | {result}: {message}"),
        Event::WaveFinished {
            wave,
            total,
            successful,
            remaining,
        } => format!(
            "wave {wave} of {total} done: {successful} successful, {remaining} repos remaining"
        ),
        Event::RateLimited {
            host,
            wait_ms,
            reason,
        } => format!(
            "{host} | rate limited, waiting {}s: {reason}",
            wait_ms / 1000
        ),
        Event::HostRetry {
            repo,
            host,
            operation,
            attempt,
            delay_ms,
            error,
        } => format!(
            "{repo} | {host} {operation} failed (attempt {attempt}), retrying in {}s: {error}",
            delay_ms / 1000
        ),
        Event::CommandIdle { label, idle_s, .. } => {
            format!("{label} | no output for {idle_s}s")
        }
        Event::PermissionsEffective {
            repo,
            agent,
            commands,
            web,
            directories,
            unrestricted,
            model,
        } => format!(
            "{repo} | agent {agent}{} grants: commands [{}], web {web}, directories [{}]{}",
            model
                .as_ref()
                .map_or_else(String::new, |model| format!(" model {model}")),
            commands.join(", "),
            directories.join(", "),
            if *unrestricted { ", unrestricted" } else { "" }
        ),
        Event::CommandFinished {
            label,
            outcome,
            duration_ms,
            ..
        } => {
            let seconds = *duration_ms as f64 / 1000.0;
            let what = match outcome {
                Outcome::Exited { code: Some(code) } => format!("exit {code}"),
                Outcome::Exited { code: None } => "killed by signal".to_string(),
                Outcome::TimedOut => "timed out, process tree terminated".to_string(),
                Outcome::Interrupted => "interrupted, process tree terminated".to_string(),
                Outcome::WaitingForInput => {
                    "waiting for input, process tree terminated".to_string()
                }
                Outcome::SpawnFailed { message } => format!("not started: {message}"),
                Outcome::Error { message } => format!("supervision error: {message}"),
            };
            format!("{label} | {what} ({seconds:.1}s)")
        }
    }
}

/// Prints events as plain lines until the bus is dropped.
pub fn print_plain(events: Receiver<Event>) {
    for event in events {
        println!("{}", plain_line(&event));
    }
}

/// How a run shows its progress while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Nothing while running (tests, library use).
    Quiet,
    /// One plain line per event on stderr; stdout stays for the final table.
    Plain,
    /// The live terminal view with this many output lines per repo.
    Live { tail: usize },
}

/// Starts the view on the event stream. Returns the thread to join once the stream ended.
pub fn spawn_view(events: Receiver<Event>, view: View) -> Option<std::thread::JoinHandle<()>> {
    match view {
        View::Quiet => None,
        View::Plain => Some(std::thread::spawn(move || {
            for event in events {
                eprintln!("{}", plain_line(&event));
            }
        })),
        View::Live { tail } => Some(std::thread::spawn(move || {
            if let Err(err) = crate::live::run(events, tail) {
                eprintln!("fleetify: live view stopped: {err}");
            }
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_line_names_the_outcome() {
        let line = plain_line(&Event::CommandFinished {
            command: 1,
            label: "repo-a".into(),
            outcome: Outcome::Exited { code: Some(2) },
            duration_ms: 1500,
        });
        assert_eq!(line, "repo-a | exit 2 (1.5s)");
    }

    #[test]
    fn permissions_line_names_the_grants() {
        let line = plain_line(&Event::PermissionsEffective {
            repo: "github.com/o/r".to_string(),
            agent: "opencode".to_string(),
            commands: vec!["cargo".to_string()],
            web: false,
            directories: Vec::new(),
            unrestricted: false,
            model: None,
        });
        assert_eq!(
            line,
            "github.com/o/r | agent opencode grants: commands [cargo], web false, directories []"
        );
    }

    #[test]
    fn output_line_is_prefixed_with_label() {
        let line = plain_line(&Event::OutputLine {
            command: 1,
            label: "repo-a".into(),
            stream: crate::events::Stream::Stdout,
            line: "hello".into(),
        });
        assert_eq!(line, "repo-a | hello");
    }
}
