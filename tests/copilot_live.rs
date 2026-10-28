//! `check-agent copilot` against the real Copilot CLI on this machine. It needs a Copilot
//! account and uses a few requests, so it is ignored by default:
//!
//!     cargo test --test copilot_live -- --ignored --nocapture
//!
//! It is not part of CI, which has no Copilot login.
#![cfg(unix)]

use std::fs;

use fleetify::agents::{check, is_checked};

#[test]
#[ignore = "needs the Copilot CLI and an account; uses requests"]
fn check_agent_copilot_against_the_real_cli() {
    let base = std::env::temp_dir().join(format!("fleetify-copilot-live-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    check(&base, "copilot", None, None).expect("the real Copilot CLI is confined to its workspace");
    assert!(is_checked(&base, "copilot", None, None));
    let _ = fs::remove_dir_all(&base);
}
