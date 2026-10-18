// Imports used only by the Unix-gated tests are gated too, so Windows builds stay warning-free.
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
use fleetify::events::Stream;
use fleetify::events::{Event, EventBus, Outcome, Redactor};
#[cfg(unix)]
use fleetify::runner::EnvPolicy;
use fleetify::runner::{RunRequest, Runner};

fn runner() -> (Runner, EventBus) {
    let bus = EventBus::new();
    (Runner::new(bus.clone(), Redactor::default()), bus)
}

fn argv(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn drain(rx: &std::sync::mpsc::Receiver<Event>) -> Vec<Event> {
    rx.try_iter().collect()
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => !stat.rsplit(") ").next().unwrap_or("").starts_with('Z'),
        Err(_) => false,
    }
}

#[cfg(unix)]
#[test]
fn captures_output_and_emits_started_output_finished_in_order() {
    let (runner, bus) = runner();
    let rx = bus.subscribe();
    let result = runner.run(RunRequest::new("repo-a", argv(&["printf", "one\\ntwo\\n"])));

    assert_eq!(result.outcome, Outcome::Exited { code: Some(0) });
    assert!(result.is_success());
    assert_eq!(result.text(), "one\ntwo");

    let events = drain(&rx);
    assert!(matches!(events.first(), Some(Event::CommandStarted { .. })));
    assert!(matches!(events.last(), Some(Event::CommandFinished { .. })));
    let lines: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::OutputLine { line, label, .. } => Some((label.clone(), line.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        lines,
        vec![
            ("repo-a".into(), "one".into()),
            ("repo-a".into(), "two".into())
        ]
    );
}

#[cfg(unix)]
#[test]
fn nonzero_exit_and_stderr_are_reported() {
    let (runner, _bus) = runner();
    let result = runner.run(RunRequest::new(
        "r",
        argv(&["sh", "-c", "echo bad 1>&2; exit 3"]),
    ));
    assert_eq!(result.outcome, Outcome::Exited { code: Some(3) });
    assert!(!result.is_success());
    assert_eq!(result.lines.len(), 1);
    assert_eq!(result.lines[0].stream, Stream::Stderr);
    assert_eq!(result.lines[0].text, "bad");
}

#[cfg(unix)]
#[test]
fn output_of_a_crashing_process_is_kept() {
    let (runner, _bus) = runner();
    let result = runner.run(RunRequest::new(
        "r",
        argv(&["sh", "-c", "echo before crash; kill -9 $$"]),
    ));
    assert_eq!(result.outcome, Outcome::Exited { code: None });
    assert_eq!(result.text(), "before crash");
}

#[test]
fn missing_program_is_a_spawn_failure_with_events() {
    let (runner, bus) = runner();
    let rx = bus.subscribe();
    let result = runner.run(RunRequest::new(
        "r",
        argv(&["fleetify-definitely-not-a-program-xyz"]),
    ));
    assert!(matches!(result.outcome, Outcome::SpawnFailed { .. }));
    let events = drain(&rx);
    assert!(matches!(events.last(), Some(Event::CommandFinished { .. })));
}

#[cfg(unix)]
#[test]
fn placeholder_values_stay_one_argument() {
    let (runner, _bus) = runner();
    let result = runner.run(RunRequest::new(
        "r",
        argv(&["printf", "[%s]", "a b;echo injected"]),
    ));
    assert_eq!(result.text(), "[a b;echo injected]");
}

#[cfg(unix)]
#[test]
fn secrets_are_redacted_in_events_and_output_but_not_in_the_process() {
    let bus = EventBus::new();
    let runner = Runner::new(bus.clone(), Redactor::new(["s3cret-value"]));
    let rx = bus.subscribe();
    let result = runner.run(RunRequest::new(
        "r",
        argv(&["printf", "%s", "s3cret-value"]),
    ));

    // The process itself received the real value.
    assert_eq!(result.text(), "***");
    let events = drain(&rx);
    let serialised = serde_json::to_string(&events).unwrap();
    assert!(!serialised.contains("s3cret-value"), "{serialised}");
}

#[cfg(unix)]
#[test]
fn minimal_env_drops_variables_that_are_not_allowlisted() {
    // SAFETY: the variable name is unique to this test and no other test reads it,
    // so no concurrent reader can observe the write.
    unsafe { std::env::set_var("FLEETIFY_RUNNER_TEST_VAR", "visible-to-parent") };
    let (runner, _bus) = runner();

    let mut restricted = RunRequest::new(
        "r",
        argv(&["sh", "-c", "echo ${FLEETIFY_RUNNER_TEST_VAR:-unset}"]),
    );
    restricted.env = EnvPolicy::minimal();
    assert_eq!(runner.run(restricted).text(), "unset");

    let inherited = RunRequest::new(
        "r",
        argv(&["sh", "-c", "echo ${FLEETIFY_RUNNER_TEST_VAR:-unset}"]),
    );
    assert_eq!(runner.run(inherited).text(), "visible-to-parent");
}

#[cfg(unix)]
#[test]
fn timeout_terminates_the_whole_process_tree() {
    let dir = std::env::temp_dir().join(format!("fleetify-runner-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let pidfile: PathBuf = dir.join("grandchild.pid");
    let script = format!("sleep 60 & echo $! > '{}'; wait", pidfile.display());

    let (runner, _bus) = runner();
    let mut request = RunRequest::new("r", argv(&["sh", "-c", &script]));
    request.timeout = Some(Duration::from_secs(2));
    let started = Instant::now();
    let result = runner.run(request);

    assert_eq!(result.outcome, Outcome::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(20));

    let pid: u32 = fs::read_to_string(&pidfile)
        .expect("grandchild pid was written")
        .trim()
        .parse()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while process_alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!process_alive(pid), "grandchild {pid} survived the timeout");
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn working_directory_is_applied() {
    let (runner, _bus) = runner();
    let dir = std::env::temp_dir();
    let mut request = RunRequest::new("r", argv(&["pwd", "-P"]));
    request.cwd = Some(dir.clone());
    let result = runner.run(request);
    assert!(result.is_success(), "{:?}", result.outcome);
    let reported = PathBuf::from(result.text().trim());
    assert_eq!(
        reported.canonicalize().unwrap(),
        dir.canonicalize().unwrap()
    );
}
