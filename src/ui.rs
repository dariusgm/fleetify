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
