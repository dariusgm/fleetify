//! Per-repo state and logs, written by one event consumer (docs/design/events-and-report.md, "Events,
//! renderers and the report"). `state.json` is replaced atomically so another
//! terminal (`fleetify status`) can read it at any time. `output.log` collects
//! every command line and output line of the repo. `events.jsonl` gets every event.

use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::thread::{self, JoinHandle};

use serde::{Deserialize, Serialize};

use crate::clock::unix_now;
use crate::events::Event;
use crate::run::RunLayout;

/// Output lines kept in `state.json` for `fleetify status`.
const LAST_LINES: usize = 10;

/// State of one repo while and after it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoState {
    pub repo: String,
    pub phase: String,
    pub result: Option<String>,
    pub started_unix: u64,
    pub updated_unix: u64,
    pub last_output_unix: Option<u64>,
    pub last_lines: Vec<String>,
}

impl RepoState {
    fn new(repo: &str, now: u64) -> Self {
        RepoState {
            repo: repo.to_string(),
            phase: "queued".to_string(),
            result: None,
            started_unix: now,
            updated_unix: now,
            last_output_unix: None,
            last_lines: Vec::new(),
        }
    }
}

/// Replaces `path` with `bytes` in one step: write a temp file, then rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

/// `state.json` of a repo key (`host/path`) inside a run.
pub fn state_path(layout: &RunLayout, repo_key: &str) -> PathBuf {
    let (host, path) = repo_key.split_once('/').unwrap_or((repo_key, ""));
    layout.root.join(host).join(path).join("state.json")
}

/// `output.log` of a repo key inside a run.
pub fn output_path(layout: &RunLayout, repo_key: &str) -> PathBuf {
    state_path(layout, repo_key).with_file_name("output.log")
}

/// Applies events to per-repo state and logs.
pub struct Recorder {
    layout: RunLayout,
    states: BTreeMap<String, RepoState>,
    recent: BTreeMap<String, VecDeque<String>>,
    outputs: BTreeMap<String, File>,
}

impl Recorder {
    pub fn new(layout: RunLayout) -> Self {
        Recorder {
            layout,
            states: BTreeMap::new(),
            recent: BTreeMap::new(),
            outputs: BTreeMap::new(),
        }
    }

    /// Updates state and logs for one event. Write errors are logged, not fatal:
    /// the event log and the report stay authoritative.
    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::CommandStarted { label, argv, .. } => {
                self.touch(label);
                self.append_output(label, &format!("$ {}", argv.join(" ")));
            }
            Event::OutputLine { label, line, .. } => {
                let now = unix_now();
                self.touch(label);
                self.append_output(label, line);
                let recent = self.recent.entry(label.clone()).or_default();
                recent.push_back(line.clone());
                while recent.len() > LAST_LINES {
                    recent.pop_front();
                }
                if let Some(state) = self.states.get_mut(label) {
                    state.last_output_unix = Some(now);
                    state.last_lines = recent.iter().cloned().collect();
                }
                self.save(label);
            }
            Event::CommandFinished { label, .. } => {
                self.touch(label);
                self.save(label);
            }
            Event::RepoPhase { repo, phase } => {
                self.touch(repo);
                if let Some(state) = self.states.get_mut(repo) {
                    state.phase = phase.clone();
                }
                self.save(repo);
            }
            Event::RepoFinished {
                repo,
                result,
                message,
            } => {
                self.touch(repo);
                if let Some(state) = self.states.get_mut(repo) {
                    state.phase = "finished".to_string();
                    state.result = Some(result.clone());
                }
                self.append_output(repo, &format!("result {result}: {message}"));
                self.save(repo);
            }
            // A wave boundary belongs to the run, not to one repo.
            Event::WaveFinished { .. } => {}
            Event::CommandIdle { .. } => {}
            Event::PermissionsEffective { .. } => {}
            Event::NetworkDenied { .. } => {}
            // The event log keeps retries; the state file only shows the phase and output.
            Event::HostRetry { .. } => {}
            Event::RateLimited { .. } => {}
            Event::CiDispatched { .. } => {}
            Event::CiFixRound { .. } => {}
        }
    }

    fn touch(&mut self, repo: &str) {
        let now = unix_now();
        let state = self
            .states
            .entry(repo.to_string())
            .or_insert_with(|| RepoState::new(repo, now));
        state.updated_unix = now;
    }

    fn save(&self, repo: &str) {
        let Some(state) = self.states.get(repo) else {
            return;
        };
        let path = state_path(&self.layout, repo);
        match serde_json::to_vec_pretty(state) {
            Ok(bytes) => {
                if let Err(err) = write_atomic(&path, &bytes) {
                    tracing::warn!(%repo, error = %err, "cannot write state");
                }
            }
            Err(err) => tracing::warn!(%repo, error = %err, "cannot serialise state"),
        }
    }

    fn append_output(&mut self, repo: &str, line: &str) {
        if !self.outputs.contains_key(repo) {
            let path = output_path(&self.layout, repo);
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(file) => {
                    self.outputs.insert(repo.to_string(), file);
                }
                Err(err) => {
                    tracing::warn!(%repo, error = %err, "cannot open output log");
                    return;
                }
            }
        }
        if let Some(file) = self.outputs.get_mut(repo) {
            let _ = writeln!(file, "{line}");
        }
    }
}

/// Consumes events until the bus is dropped: writes `events.jsonl` and feeds the recorder.
pub fn spawn_recorder(events: Receiver<Event>, layout: RunLayout) -> io::Result<JoinHandle<()>> {
    let mut log = File::create(layout.events_path())?;
    let mut recorder = Recorder::new(layout);
    Ok(thread::spawn(move || {
        for event in events {
            if let Ok(line) = serde_json::to_string(&event) {
                let _ = writeln!(log, "{line}");
            }
            recorder.apply(&event);
        }
    }))
}

/// All repo states of a run, found by walking the run folder.
pub fn read_states(run_root: &Path) -> io::Result<Vec<RepoState>> {
    let mut found = Vec::new();
    collect_states(run_root, &mut found)?;
    found.sort_by(|a, b| a.repo.cmp(&b.repo));
    Ok(found)
}

fn collect_states(dir: &Path, found: &mut Vec<RepoState>) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let path = entry?.path();
        if path.is_dir() && path.file_name().is_none_or(|n| n != "workspace") {
            collect_states(&path, found)?;
        } else if path.file_name().is_some_and(|n| n == "state.json")
            && let Ok(text) = fs::read_to_string(&path)
            && let Ok(state) = serde_json::from_str::<RepoState>(&text)
        {
            found.push(state);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Stream;

    #[test]
    fn state_follows_phases_output_and_result() {
        let root = std::env::temp_dir().join(format!("fleetify-state-unit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let layout = RunLayout::new(&root, "r1");
        let mut recorder = Recorder::new(layout.clone());
        let repo = "github.com/a/b";
        recorder.apply(&Event::RepoPhase {
            repo: repo.into(),
            phase: "cloning".into(),
        });
        recorder.apply(&Event::OutputLine {
            command: 1,
            label: repo.into(),
            stream: Stream::Stdout,
            line: "fetching".into(),
        });
        let state: RepoState =
            serde_json::from_str(&fs::read_to_string(state_path(&layout, repo)).unwrap()).unwrap();
        assert_eq!(state.phase, "cloning");
        assert_eq!(state.last_lines, vec!["fetching".to_string()]);
        assert!(state.last_output_unix.is_some());

        recorder.apply(&Event::RepoFinished {
            repo: repo.into(),
            result: "dry_run".into(),
            message: "ok".into(),
        });
        let states = read_states(&layout.root).unwrap();
        assert_eq!(states[0].result.as_deref(), Some("dry_run"));
        assert_eq!(states[0].phase, "finished");
        let log = fs::read_to_string(output_path(&layout, repo)).unwrap();
        assert!(log.contains("fetching") && log.contains("result dry_run"));
        let _ = fs::remove_dir_all(&root);
    }
}
