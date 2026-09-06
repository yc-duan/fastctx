//! A single cancellable local helper job, polled without blocking terminal input or drawing.

use crate::control::paths::ControlPaths;
use crate::world::control::{Action, Failure, FailureKind, Reply};
use std::io::{Read, Seek, SeekFrom, Write};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

pub(super) struct Worker {
    receiver: mpsc::Receiver<Reply>,
    cancel: Arc<AtomicBool>,
    pub(super) started: Instant,
    pub(super) read_only: bool,
}

impl Worker {
    pub(super) fn spawn(paths: &ControlPaths, action: Action) -> Result<Self, Failure> {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::clone(&cancel);
        let read_only = action.is_read();
        let paths = paths.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("fastctx-world-control".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(&paths, &action, &cancelled)
                }));
                let reply = match result {
                    Ok(Ok(reply)) => reply,
                    Ok(Err(failure)) => Reply::Failed(failure),
                    Err(_) => Reply::Failed(Failure {
                        kind: FailureKind::Other,
                        detail: "The local World worker stopped unexpectedly.".into(),
                        uncertain: !action.is_read(),
                    }),
                };
                let _ = sender.send(reply);
            })
            .map_err(|error| {
                Failure::new(format!("Cannot start the local World worker: {error}"))
            })?;
        Ok(Self {
            receiver,
            cancel,
            started: Instant::now(),
            read_only,
        })
    }
    pub(super) fn poll(&self) -> Option<Reply> {
        match self.receiver.try_recv() {
            Ok(reply) => Some(reply),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Reply::Failed(Failure {
                kind: FailureKind::Other,
                detail: "The local World worker disconnected.".into(),
                uncertain: !self.read_only,
            })),
        }
    }
    pub(super) fn cancel_read(&self) {
        if self.read_only {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel_read();
    }
}

fn run(paths: &ControlPaths, action: &Action, cancelled: &AtomicBool) -> Result<Reply, Failure> {
    let executable = std::env::current_exe()
        .map_err(|error| Failure::new(format!("Cannot locate FastCtx: {error}")))?;
    let directory = crate::edit::private_storage::control_center_directory().join("world-control");
    crate::edit::private_storage::ensure_private_directory(&directory, "World control capture")
        .map_err(Failure::from)?;
    let mut stdout = tempfile::tempfile_in(&directory).map_err(|error| {
        Failure::new(format!("Cannot open the private control result: {error}"))
    })?;
    let mut stderr = tempfile::tempfile_in(&directory).map_err(|error| {
        Failure::new(format!(
            "Cannot open the private control diagnostic: {error}"
        ))
    })?;
    let mut command = crate::process_policy::noninteractive_command(executable);
    command
        .arg("world-control")
        .env("HOME", &paths.home)
        .stdin(Stdio::piped())
        .stdout(stdout.try_clone().map_err(|error| {
            Failure::new(format!(
                "Cannot duplicate the control result handle: {error}"
            ))
        })?)
        .stderr(stderr.try_clone().map_err(|error| {
            Failure::new(format!(
                "Cannot duplicate the control diagnostic handle: {error}"
            ))
        })?);
    let mut child = command
        .spawn()
        .map_err(|error| Failure::new(format!("Cannot start the World helper: {error}")))?;
    let mut request = serde_json::to_vec(action)
        .map_err(|error| Failure::new(format!("Cannot encode the World request: {error}")))?;
    let sent = child
        .stdin
        .take()
        .ok_or_else(|| Failure::new("The World helper has no input pipe."))?
        .write_all(&request);
    use zeroize::Zeroize;
    request.zeroize();
    if let Err(error) = sent {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Failure::new(format!(
            "Cannot send the World request: {error}"
        )));
    }
    let deadline = Instant::now() + action.timeout();
    let mut stopped = None;
    let status = loop {
        if cancelled.load(Ordering::Relaxed) && action.is_read() {
            stopped = Some(Failure {
                kind: FailureKind::Unavailable,
                detail: "The read was cancelled.".into(),
                uncertain: false,
            });
            let _ = child.kill();
            break child.wait();
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Failure::new(format!(
                    "Cannot observe the World helper: {error}"
                )));
            }
        }
        if Instant::now() >= deadline {
            stopped = Some(Failure {
                kind: FailureKind::Timeout,
                detail: format!(
                    "The local World operation did not finish within {} seconds.",
                    action.timeout().as_secs()
                ),
                uncertain: !action.is_read(),
            });
            let _ = child.kill();
            break child.wait();
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    .map_err(|error| Failure::new(format!("Cannot collect the World helper exit: {error}")))?;
    if let Some(failure) = stopped {
        return Err(failure);
    }
    // File-backed capture cannot wait for EOF from an unrelated inherited pipe writer.
    stdout
        .seek(SeekFrom::Start(0))
        .and_then(|_| stderr.seek(SeekFrom::Start(0)))
        .map_err(|error| Failure::new(format!("Cannot read the control capture: {error}")))?;
    let output = read_capture(stdout, 16 * 1024 * 1024).map_err(Failure::new)?;
    let errors = read_capture(stderr, 64 * 1024).map_err(Failure::new)?;
    if !status.success() {
        return Err(Failure {
            kind: FailureKind::Other,
            detail: format!(
                "The World helper exited with {status}: {}",
                String::from_utf8_lossy(&errors)
            ),
            uncertain: !action.is_read(),
        });
    }
    serde_json::from_slice(&output).map_err(|error| Failure {
        kind: FailureKind::Other,
        detail: format!("Cannot decode the World helper result: {error}"),
        uncertain: !action.is_read(),
    })
}

fn read_capture(file: impl Read, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Cannot read the World control result: {error}"))?;
    if bytes.len() > limit {
        Err("The World helper output exceeded its bounded local frame.".into())
    } else {
        Ok(bytes)
    }
}
