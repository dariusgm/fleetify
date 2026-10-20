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

/// Asks for the items of one commit until the verdict is final or `timeout` is over.
/// `fetch` must answer for that commit only. A timeout while items are still pending
/// returns `Pending`; the caller maps that to `ci_timeout`.
pub fn poll(
    mut fetch: impl FnMut() -> Result<Vec<CiItem>, HostError>,
    timeout: Duration,
    interval: Duration,
) -> Result<Verdict, HostError> {
    let started = Instant::now();
    loop {
        let verdict = evaluate(&fetch()?);
        let done = !matches!(verdict, Verdict::Pending(_) | Verdict::Missing);
        if done || started.elapsed() >= timeout {
            return Ok(verdict);
        }
        thread::sleep(interval);
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
            Duration::from_secs(5),
            Duration::from_millis(1),
        )
        .unwrap();
        assert_eq!(verdict, Verdict::Passed);
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn poll_gives_up_at_the_timeout_with_pending_items() {
        let verdict = poll(
            || Ok(vec![item("test", CiState::Pending)]),
            Duration::from_millis(20),
            Duration::from_millis(1),
        )
        .unwrap();
        assert_eq!(verdict, Verdict::Pending(vec!["test".to_string()]));
    }
}
