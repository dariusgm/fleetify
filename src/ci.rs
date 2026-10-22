//! CI providers and the CI gate (milestone M11).
//!
//! The gate judges one commit: the SHA fleetify pushed. It never looks up a branch or
//! a pull request, because a later push to the same branch has its own CI items and
//! must not change the answer for this commit.

use std::thread;
use std::time::{Duration, Instant};

use crate::hosts::{CiItem, CiState, HostError};

/// The verdict for one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Every item completed with success.
    Passed,
    /// At least one item failed. Lists the failed items.
    Failed(Vec<String>),
    /// Nothing failed, but an item waits for a person. Lists them. Not waited for.
    Blocked(Vec<String>),
    /// Nothing failed, but some items are still running. Lists them.
    Pending(Vec<String>),
    /// The commit has no CI items at all.
    Missing,
}

/// Judges the items of one commit. A failure is final even while others still run.
pub fn evaluate(items: &[CiItem]) -> Verdict {
    if items.is_empty() {
        return Verdict::Missing;
    }
    let failed: Vec<String> = names(items, CiState::Failure);
    if !failed.is_empty() {
        return Verdict::Failed(failed);
    }
    let blocked = names(items, CiState::Blocked);
    if !blocked.is_empty() {
        return Verdict::Blocked(blocked);
    }
    let pending = names(items, CiState::Pending);
    if !pending.is_empty() {
        return Verdict::Pending(pending);
    }
    Verdict::Passed
}

fn names(items: &[CiItem], state: CiState) -> Vec<String> {
    items
        .iter()
        .filter(|item| item.state == state)
        .map(|item| item.name.clone())
        .collect()
}

/// The longest wait between two tries while a host is failing.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(300);

/// The wait before retry number `attempt` (1 for the first retry): `interval`, doubled
/// for each further attempt, capped at `MAX_RETRY_DELAY`.
pub fn backoff(interval: Duration, attempt: u32) -> Duration {
    let factor = 2u32.saturating_pow(attempt.saturating_sub(1));
    interval.saturating_mul(factor).min(MAX_RETRY_DELAY)
}

/// When fleetify starts CI itself: the commit has no items after `after`.
pub struct Dispatch<'a> {
    /// The wait for the host's own CI. Zero starts CI before the first read.
    pub after: Duration,
    /// Starts CI for the pushed branch. Called at most once per poll.
    pub start: &'a mut dyn FnMut() -> Result<(), HostError>,
}

/// Asks for the items of one commit until the verdict is final or `timeout` is over.
/// `fetch` must answer for that commit only.
///
/// A transient failure (no answer, 5xx, 429) is retried with back-off and reported through
/// `on_retry`. If the budget runs out while the host still fails, the last error is returned
/// and the caller maps it to `host_unreachable`. Any other error is returned at once. A
/// timeout while items are still pending returns `Pending`; the caller maps that to
/// `ci_timeout`. A poll never returns a verdict built from a failed read.
///
/// With `dispatch`, CI is started once the commit still has no items after `dispatch.after`
/// (at once when that is zero). A start error is returned as is.
pub fn poll(
    mut fetch: impl FnMut() -> Result<Vec<CiItem>, HostError>,
    mut on_retry: impl FnMut(&HostError, u32, Duration),
    timeout: Duration,
    interval: Duration,
    mut dispatch: Option<Dispatch<'_>>,
) -> Result<Verdict, HostError> {
    let started = Instant::now();
    let mut attempt = 0;
    let mut dispatched = false;
    if let Some(d) = dispatch.as_mut()
        && d.after.is_zero()
    {
        (d.start)()?;
        dispatched = true;
    }
    loop {
        match fetch() {
            Ok(items) => {
                attempt = 0;
                let verdict = evaluate(&items);
                if matches!(verdict, Verdict::Missing)
                    && !dispatched
                    && let Some(d) = dispatch.as_mut()
                    && started.elapsed() >= d.after
                {
                    (d.start)()?;
                    dispatched = true;
                }
                let done = !matches!(verdict, Verdict::Pending(_) | Verdict::Missing);
                if done || started.elapsed() >= timeout {
                    return Ok(verdict);
                }
                thread::sleep(interval);
            }
            Err(err) if err.is_transient() => {
                attempt += 1;
                let delay = backoff(interval, attempt);
                if started.elapsed().saturating_add(delay) >= timeout {
                    return Err(err);
                }
                on_retry(&err, attempt, delay);
                thread::sleep(delay);
            }
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn item(name: &str, state: CiState) -> CiItem {
        CiItem {
            name: name.to_string(),
            state,
        }
    }

    #[test]
    fn a_manual_item_blocks_the_verdict_without_waiting() {
        let items = [
            item("deploy", CiState::Blocked),
            item("test", CiState::Pending),
        ];
        assert_eq!(
            evaluate(&items),
            Verdict::Blocked(vec!["deploy".to_string()])
        );
        let failed = [
            item("deploy", CiState::Blocked),
            item("lint", CiState::Failure),
        ];
        assert_eq!(evaluate(&failed), Verdict::Failed(vec!["lint".to_string()]));
    }

    #[test]
    fn no_items_is_missing_not_passed() {
        assert_eq!(evaluate(&[]), Verdict::Missing);
    }

    #[test]
    fn all_success_passes() {
        let items = [
            item("lint", CiState::Success),
            item("test", CiState::Success),
        ];
        assert_eq!(evaluate(&items), Verdict::Passed);
    }

    #[test]
    fn a_failure_is_final_even_while_others_run() {
        let items = [
            item("lint", CiState::Failure),
            item("test", CiState::Pending),
        ];
        assert_eq!(evaluate(&items), Verdict::Failed(vec!["lint".to_string()]));
    }

    #[test]
    fn pending_items_are_named_until_they_finish() {
        let items = [
            item("test", CiState::Pending),
            item("lint", CiState::Success),
        ];
        assert_eq!(evaluate(&items), Verdict::Pending(vec!["test".to_string()]));
    }

    #[test]
    fn poll_returns_the_final_verdict_of_the_commit() {
        let calls = Cell::new(0);
        let verdict = poll(
            || {
                calls.set(calls.get() + 1);
                Ok(if calls.get() < 3 {
                    vec![item("test", CiState::Pending)]
                } else {
                    vec![item("test", CiState::Success)]
                })
            },
            |_, _, _| {},
            Duration::from_secs(5),
            Duration::from_millis(1),
            None,
        )
        .unwrap();
        assert_eq!(verdict, Verdict::Passed);
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn poll_gives_up_at_the_timeout_with_pending_items() {
        let verdict = poll(
            || Ok(vec![item("test", CiState::Pending)]),
            |_, _, _| {},
            Duration::from_millis(20),
            Duration::from_millis(1),
            None,
        )
        .unwrap();
        assert_eq!(verdict, Verdict::Pending(vec!["test".to_string()]));
    }

    #[test]
    fn backoff_doubles_and_is_capped() {
        assert_eq!(backoff(Duration::from_secs(30), 1), Duration::from_secs(30));
        assert_eq!(backoff(Duration::from_secs(30), 2), Duration::from_secs(60));
        assert_eq!(
            backoff(Duration::from_secs(30), 3),
            Duration::from_secs(120)
        );
        assert_eq!(backoff(Duration::from_secs(30), 40), MAX_RETRY_DELAY);
    }

    #[test]
    fn a_transient_failure_is_retried_and_the_verdict_still_comes() {
        let calls = Cell::new(0);
        let retries = Cell::new(0);
        let verdict = poll(
            || {
                calls.set(calls.get() + 1);
                if calls.get() <= 2 {
                    return Err(HostError::Status {
                        status: 503,
                        message: "busy".into(),
                    });
                }
                Ok(vec![item("test", CiState::Success)])
            },
            |_, attempt, _| retries.set(attempt),
            Duration::from_secs(5),
            Duration::from_millis(1),
            None,
        )
        .unwrap();
        assert_eq!(verdict, Verdict::Passed);
        assert_eq!(calls.get(), 3);
        assert_eq!(retries.get(), 2);
    }

    #[test]
    fn a_failure_that_outlasts_the_budget_returns_the_transient_error() {
        let err = poll(
            || Err(HostError::Transport("connection refused".into())),
            |_, _, _| {},
            Duration::from_millis(20),
            Duration::from_millis(1),
            None,
        )
        .unwrap_err();
        assert!(err.is_transient(), "{err:?}");
    }

    #[test]
    fn a_permanent_failure_is_not_retried() {
        let calls = Cell::new(0);
        let err = poll(
            || {
                calls.set(calls.get() + 1);
                Err(HostError::Status {
                    status: 401,
                    message: "bad token".into(),
                })
            },
            |_, _, _| panic!("no retry for a permanent error"),
            Duration::from_secs(5),
            Duration::from_millis(1),
            None,
        )
        .unwrap_err();
        assert!(!err.is_transient());
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn no_items_after_the_wait_starts_ci_once() {
        let started = Cell::new(0);
        let mut start = || {
            started.set(started.get() + 1);
            Ok(())
        };
        let verdict = poll(
            || Ok(Vec::new()),
            |_, _, _| {},
            Duration::from_millis(60),
            Duration::from_millis(1),
            Some(Dispatch {
                after: Duration::from_millis(10),
                start: &mut start,
            }),
        )
        .unwrap();
        assert_eq!(verdict, Verdict::Missing);
        assert_eq!(started.get(), 1);
    }

    #[test]
    fn a_zero_wait_starts_ci_before_the_first_read() {
        let order = std::cell::RefCell::new(Vec::new());
        let mut start = || {
            order.borrow_mut().push("start");
            Ok(())
        };
        let verdict = poll(
            || {
                order.borrow_mut().push("read");
                Ok(vec![item("test", CiState::Success)])
            },
            |_, _, _| {},
            Duration::from_secs(5),
            Duration::from_millis(1),
            Some(Dispatch {
                after: Duration::ZERO,
                start: &mut start,
            }),
        )
        .unwrap();
        assert_eq!(verdict, Verdict::Passed);
        assert_eq!(*order.borrow(), vec!["start", "read"]);
    }

    #[test]
    fn items_within_the_wait_mean_no_start() {
        let reads = Cell::new(0);
        let started = Cell::new(0);
        let mut start = || {
            started.set(started.get() + 1);
            Ok(())
        };
        let verdict = poll(
            || {
                reads.set(reads.get() + 1);
                Ok(if reads.get() == 1 {
                    Vec::new()
                } else {
                    vec![item("test", CiState::Success)]
                })
            },
            |_, _, _| {},
            Duration::from_secs(5),
            Duration::from_millis(1),
            Some(Dispatch {
                after: Duration::from_secs(60),
                start: &mut start,
            }),
        )
        .unwrap();
        assert_eq!(verdict, Verdict::Passed);
        assert_eq!(started.get(), 0);
    }

    #[test]
    fn a_start_error_is_returned_as_is() {
        let mut start = || Err(HostError::Unsupported("no workflow".into()));
        let err = poll(
            || Ok(Vec::new()),
            |_, _, _| {},
            Duration::from_secs(5),
            Duration::from_millis(1),
            Some(Dispatch {
                after: Duration::ZERO,
                start: &mut start,
            }),
        )
        .unwrap_err();
        assert!(matches!(err, HostError::Unsupported(_)));
    }
}
