#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::time::{Duration, Instant};

use super::*;

#[cfg(unix)]
#[test]
fn probe_reports_success_and_failure() {
    assert!(command_succeeds_within("true", &[], Duration::from_secs(5)));
    assert!(!command_succeeds_within(
        "false",
        &[],
        Duration::from_secs(5)
    ));
}

#[test]
fn probe_of_a_missing_program_fails() {
    assert!(!command_succeeds_within(
        "antre-no-such-runtime-cli",
        &["--version"],
        Duration::from_secs(5)
    ));
}

/// A hung runtime (like a stuck Docker Desktop answering `docker info`) must
/// not block startup past the timeout.
#[cfg(unix)]
#[test]
fn probe_kills_a_hung_command_at_the_deadline() {
    let started = Instant::now();
    assert!(!command_succeeds_within(
        "sleep",
        &["30"],
        Duration::from_millis(300)
    ));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "probe waited {:?}",
        started.elapsed()
    );
}

#[test]
fn runtime_probe_timeout_stays_short() {
    assert!(RUNTIME_PROBE_TIMEOUT <= Duration::from_secs(10));
}
