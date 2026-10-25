//! Running a program to its end within a deadline (`specs/world-concurrency.md`,
//! 等外面的事都有期限). Past the deadline the program is ended, only it,
//! never a process it started; its output is read while it lives and briefly
//! after, so a child that keeps the program's output open does not hold the
//! caller forever.

use std::io::{self, Read};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long the output may stay open once the program has exited, held by a
/// process it started, before what was read is taken as all of it.
const AFTER_EXIT: Duration = Duration::from_secs(2);
/// How often the program is looked at while it runs.
const LOOK: Duration = Duration::from_millis(20);

/// Runs `command` with its output piped, for at most `within`; `what` names
/// it in the error past the deadline and in what the process says it waits
/// for. Its standard input is the caller's to set.
pub fn output(command: Command, what: &str, within: Duration) -> io::Result<Output> {
    output_tracked(command, what, within).map_err(|failure| failure.error)
}

/// A failed invocation, with evidence of whether the process was started.
#[derive(Debug)]
pub struct OutputFailure {
    pub error: io::Error,
    pub started: bool,
}

/// Like `output`, preserving the spawn boundary for callers that must not
/// repeat a possibly accepted side effect.
pub fn output_tracked(command: Command, what: &str, within: Duration) -> Result<Output, OutputFailure> {
    let mut started = false;
    output_inner(command, what, within, &mut started).map_err(|error| OutputFailure { error, started })
}

fn output_inner(mut command: Command, what: &str, within: Duration, spawned: &mut bool) -> io::Result<Output> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let _waiting = crate::doing::waiting_outside(what, within);
    let mut child = command.spawn()?;
    *spawned = true;
    let stdout = child.stdout.take().map(read);
    let stderr = child.stderr.take().map(read);
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= within {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{what} did not finish within {} s", within.as_secs()),
            ));
        }
        std::thread::sleep(LOOK);
    };
    let until = Instant::now() + AFTER_EXIT;
    Ok(Output { status, stdout: collect(stdout, until), stderr: collect(stderr, until) })
}

/// Whether `error` is `output`'s, for a program that ran past its deadline.
pub fn ran_late(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::TimedOut
}

/// Reads a pipe on a thread of its own, a part at a time as it comes.
fn read(mut pipe: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (parts, received) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = vec![0; 8192];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    if parts.send(buffer[..n].to_vec()).is_err() {
                        return;
                    }
                }
            }
        }
    });
    received
}

/// What was read of a pipe until it closed, or until `until`.
fn collect(pipe: Option<mpsc::Receiver<Vec<u8>>>, until: Instant) -> Vec<u8> {
    let Some(pipe) = pipe else { return Vec::new() };
    let mut bytes = Vec::new();
    loop {
        let left = until.saturating_duration_since(Instant::now());
        match pipe.recv_timeout(left) {
            Ok(part) => bytes.extend_from_slice(&part),
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => return bytes,
        }
    }
}
