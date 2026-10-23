//! A program's exit and both output streams share one deadline.

use std::fs::File;
use std::io::{self, Read as _};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use anyhow::Context as _;

#[cfg(unix)]
use std::os::fd::{AsRawFd as _, OwnedFd as OwnedPipe};
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle as _, OwnedHandle as OwnedPipe};

/// Includes starting the program and reading its complete answer. Its first
/// run on a slow machine can wait while the system looks it over.
const ANSWERS_WITHIN: Duration = Duration::from_secs(120);

pub(crate) fn answer(command: &mut Command, path: &Path) -> anyhow::Result<Output> {
    let deadline = Instant::now() + ANSWERS_WITHIN;
    let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let result = (|| {
        let mut stdout = Pipe::new(child.stdout.take().context("program stdout was not piped")?)?;
        let mut stderr = Pipe::new(child.stderr.take().context("program stderr was not piped")?)?;
        loop {
            anyhow::ensure!(
                Instant::now() < deadline,
                "{} did not answer within {} seconds",
                path.display(),
                ANSWERS_WITHIN.as_secs()
            );
            let out = stdout.read().context("reading program stdout")?;
            let err = stderr.read().context("reading program stderr")?;
            if let Some(status) = child.try_wait()?
                && stdout.ended
                && stderr.ended
            {
                return Ok(Output { status, stdout: stdout.bytes, stderr: stderr.bytes });
            }
            if !out && !err {
                std::thread::sleep(Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())));
            }
        }
    })();
    if result.is_err() && child.try_wait()?.is_none() {
        child.kill().context("ending the program after its answer failed")?;
        child.wait().context("waiting for the ended program")?;
    }
    result
}

struct Pipe {
    file: File,
    bytes: Vec<u8>,
    ended: bool,
}

impl Pipe {
    fn new(pipe: impl Into<OwnedPipe>) -> io::Result<Self> {
        let file = File::from(pipe.into());
        #[cfg(unix)]
        {
            // SAFETY: this is the owned read end; no other reader uses it.
            let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
            if flags == -1 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: the descriptor is still owned here; its other flags stay set.
            if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Self { file, bytes: Vec::new(), ended: false })
    }

    /// Reads at most one buffer so even continuous output cannot hide the
    /// deadline. No reader thread or inherited writer can hold up the caller.
    fn read(&mut self) -> io::Result<bool> {
        if self.ended {
            return Ok(false);
        }
        let mut buffer = [0; 8192];
        let buffer = buffer.as_mut_slice();
        #[cfg(windows)]
        let buffer = {
            use windows_sys::Win32::Foundation::ERROR_BROKEN_PIPE;
            use windows_sys::Win32::System::Pipes::PeekNamedPipe;
            let mut queued = 0;
            // SAFETY: a live pipe read end and a valid out-parameter. This is
            // its sole reader, so the queued bytes remain available to read.
            if unsafe {
                PeekNamedPipe(
                    self.file.as_raw_handle(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut queued,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_BROKEN_PIPE as i32) {
                    return Err(error);
                }
                self.ended = true;
                return Ok(false);
            }
            if queued == 0 {
                return Ok(false);
            }
            let count = buffer.len().min(queued as usize);
            &mut buffer[..count]
        };
        match self.file.read(buffer) {
            Ok(0) => {
                self.ended = true;
                Ok(false)
            }
            Ok(count) => {
                self.bytes.extend_from_slice(&buffer[..count]);
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(false),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(true),
            Err(error) => Err(error),
        }
    }
}
