//! The live terminal view (milestone M13). It draws the event stream as a table of
//! repos with their phase and result, and the last output lines of the repos that run.
//! It only reads events; it never changes the run. Ctrl-C in the view is the same as
//! Ctrl-C in the terminal: the running commands are stopped.

use std::collections::BTreeMap;
use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use ratatui::crossterm::event::{self, Event as KeyEvent, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};
use ratatui::{DefaultTerminal, Frame};

use crate::events::Event;

const FRAME: Duration = Duration::from_millis(200);

#[derive(Default)]
struct RepoView {
    phase: String,
    result: Option<String>,
    idle_s: Option<u64>,
    tail: Vec<String>,
}

/// Everything the view shows, built from the events so far.
#[derive(Default)]
pub struct Model {
    repos: BTreeMap<String, RepoView>,
    tail_lines: usize,
}

impl Model {
    pub fn new(tail_lines: usize) -> Self {
        Model {
            repos: BTreeMap::new(),
            tail_lines,
        }
    }

    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::RepoPhase { repo, phase } => {
                let view = self.repos.entry(repo.clone()).or_default();
                view.phase = phase.clone();
                view.idle_s = None;
            }
            Event::RepoFinished { repo, result, .. } => {
                let view = self.repos.entry(repo.clone()).or_default();
                view.phase = "finished".to_string();
                view.result = Some(result.clone());
                view.idle_s = None;
            }
            Event::OutputLine { label, line, .. } => {
                let view = self.repos.entry(label.clone()).or_default();
                view.idle_s = None;
                view.tail.push(line.clone());
                let excess = view.tail.len().saturating_sub(self.tail_lines);
                view.tail.drain(..excess);
            }
            Event::CommandIdle { label, idle_s, .. } => {
                self.repos.entry(label.clone()).or_default().idle_s = Some(*idle_s);
            }
            _ => {}
        }
    }

    fn done(&self) -> usize {
        self.repos
            .values()
            .filter(|view| view.result.is_some())
            .count()
    }
}

/// Draws the view until the event stream ends. Restores the terminal on the way out.
pub fn run(events: Receiver<Event>, tail_lines: usize) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let outcome = live_loop(&mut terminal, &events, tail_lines);
    ratatui::restore();
    outcome
}

fn live_loop(
    terminal: &mut DefaultTerminal,
    events: &Receiver<Event>,
    tail_lines: usize,
) -> io::Result<()> {
    let mut model = Model::new(tail_lines);
    loop {
        terminal.draw(|frame| draw(frame, &model))?;
        match events.recv_timeout(FRAME) {
            Ok(event) => model.apply(&event),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // Drain what is queued, so a burst of output does not take many frames.
        while let Ok(event) = events.try_recv() {
            model.apply(&event);
        }
        while event::poll(Duration::ZERO)? {
            if let KeyEvent::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
                && key.code == KeyCode::Char('c')
                && key.modifiers.contains(KeyModifiers::CONTROL)
            {
                crate::runner::request_interrupt();
            }
        }
    }
    terminal.draw(|frame| draw(frame, &model))?;
    Ok(())
}

fn draw(frame: &mut Frame, model: &Model) {
    let [header, table, tails] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(model.tail_lines as u16 + 2),
    ])
    .areas(frame.area());

    let header_text = format!(
        "fleetify: {} repos, {} finished (Ctrl-C stops the run)",
        model.repos.len(),
        model.done()
    );
    frame.render_widget(
        Paragraph::new(header_text).style(Style::default().add_modifier(Modifier::BOLD)),
        header,
    );

    let rows = model.repos.iter().map(|(repo, view)| {
        let state = match (&view.result, view.idle_s) {
            (Some(result), _) => result.clone(),
            (None, Some(idle)) => format!("{} (no output {idle}s)", view.phase),
            (None, None) => view.phase.clone(),
        };
        Row::new([repo.clone(), state]).style(result_style(view.result.as_deref()))
    });
    let widths = [Constraint::Percentage(55), Constraint::Percentage(45)];
    frame.render_widget(
        Table::new(rows, widths)
            .header(Row::new(["repo", "phase / result"]).style(Modifier::BOLD))
            .block(Block::default().borders(Borders::ALL)),
        table,
    );

    // The last output lines of the repos still running, most recent repo first.
    let mut lines = Vec::new();
    for (repo, view) in model.repos.iter().filter(|(_, v)| v.result.is_none()) {
        for line in &view.tail {
            lines.push(format!("{repo} | {line}"));
        }
    }
    let shown = lines.len().saturating_sub(model.tail_lines);
    frame.render_widget(
        Paragraph::new(lines.split_off(shown).join("\n"))
            .block(Block::default().borders(Borders::ALL).title("output")),
        tails,
    );
}

/// Green for successful results, red otherwise. Plain when `NO_COLOR` is set.
fn result_style(result: Option<&str>) -> Style {
    if std::env::var_os("NO_COLOR").is_some() {
        return Style::default();
    }
    match result {
        Some(result) if crate::report::is_successful(result) => Style::default().fg(Color::Green),
        Some(_) => Style::default().fg(Color::Red),
        None => Style::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Stream;

    fn line(repo: &str, text: &str) -> Event {
        Event::OutputLine {
            command: 1,
            label: repo.to_string(),
            stream: Stream::Stdout,
            line: text.to_string(),
        }
    }

    #[test]
    fn the_tail_keeps_only_the_last_lines_per_repo() {
        let mut model = Model::new(2);
        for text in ["one", "two", "three"] {
            model.apply(&line("github.com/o/r", text));
        }
        assert_eq!(model.repos["github.com/o/r"].tail, ["two", "three"]);
    }

    #[test]
    fn a_finished_repo_counts_and_keeps_its_result() {
        let mut model = Model::new(3);
        model.apply(&Event::RepoPhase {
            repo: "github.com/o/r".into(),
            phase: "verifying".into(),
        });
        assert_eq!(model.done(), 0);
        model.apply(&Event::RepoFinished {
            repo: "github.com/o/r".into(),
            result: "pr_open".into(),
            message: "pushed".into(),
        });
        assert_eq!(model.done(), 1);
        assert_eq!(model.repos["github.com/o/r"].phase, "finished");
    }

    #[test]
    fn idle_is_shown_until_output_or_a_new_phase_arrives() {
        let mut model = Model::new(3);
        model.apply(&Event::CommandIdle {
            command: 1,
            label: "github.com/o/r".into(),
            idle_s: 900,
        });
        assert_eq!(model.repos["github.com/o/r"].idle_s, Some(900));
        model.apply(&line("github.com/o/r", "still going"));
        assert_eq!(model.repos["github.com/o/r"].idle_s, None);
    }
}
