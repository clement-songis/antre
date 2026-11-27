//! Bounded external command runs for runtime probes (`docker ps`,
//! `docker inspect`, `docker info`).
//!
//! An unresponsive Docker daemon keeps CLI calls blocked indefinitely; every
//! probe must give up after a deadline instead of hanging its caller.

use std::{
    io::Read,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use tracing::warn;

/// Deadline for a container runtime CLI call made while probing.
pub const RUNTIME_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

const POLL_INTERVAL: Duration = Duration::from_millis(20);

fn read_all(pipe: Option<impl Read + Send + 'static>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    })
}

/// Run `program args` with stdin closed and capture its output, killing it
/// if it is still running after `timeout`. Returns `None` when the program
/// cannot be started or times out.
#[must_use]
pub fn output_within(program: &str, args: &[&str], timeout: Duration) -> Option<Output> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    // Pipes are drained on their own threads so a chatty command cannot
    // block on a full pipe while we wait for it to exit.
    let stdout = read_all(child.stdout.take());
    let stderr = read_all(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                warn!(
                    program,
                    ?args,
                    timeout_secs = timeout.as_secs_f32(),
                    "runtime command timed out; treating it as failed"
                );
                return None;
            },
            Err(_) => {
                let _ = child.kill();
                return None;
            },
        }
    };
    Some(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn captures_output_and_status() {
        let output = output_within(
            "sh",
            &["-c", "echo out; echo err >&2; exit 3"],
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(output.stdout, b"out\n");
        assert_eq!(output.stderr, b"err\n");
        assert_eq!(output.status.code(), Some(3));
    }

    #[cfg(unix)]
    #[test]
    fn large_output_does_not_deadlock() {
        let output = output_within(
            "sh",
            &["-c", "head -c 1000000 /dev/zero"],
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(output.stdout.len(), 1_000_000);
    }

    #[cfg(unix)]
    #[test]
    fn hung_command_is_killed_at_deadline() {
        let started = Instant::now();
        assert!(output_within("sleep", &["30"], Duration::from_millis(200)).is_none());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "waited {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn missing_program_is_none() {
        assert!(
            output_within("moltis-no-such-cli", &["--version"], Duration::from_secs(1)).is_none()
        );
    }
}
