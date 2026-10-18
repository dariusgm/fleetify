//! Ctrl-C handling. Kept in its own test binary: the interrupt flag is process-wide.

#[cfg(unix)]
#[test]
fn ctrl_c_terminates_a_running_process_and_reports_interrupted() {
    use std::time::{Duration, Instant};

    use fleetify::events::{EventBus, Outcome, Redactor};
    use fleetify::runner::{RunRequest, Runner, request_interrupt};

    let runner = Runner::new(EventBus::new(), Redactor::default());
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(300));
        request_interrupt();
    });
    let started = Instant::now();
    let result = runner.run(RunRequest::new("r", vec!["sleep".into(), "30".into()]));
    assert_eq!(result.outcome, Outcome::Interrupted);
    assert!(started.elapsed() < Duration::from_secs(10));
}
