//! One process and its bounded diagnostic tail. Only the request's blocking
//! task touches stdin/stdout; independent readers observe exit and stderr.

use std::collections::VecDeque;
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use super::{Failure, Settings, protocol};
use crate::child::Child;

const STDERR: usize = 4096;

pub(super) struct Process {
    pub child: Arc<Child>,
    input: io::PipeWriter,
    output: io::PipeReader,
    said: Arc<Mutex<VecDeque<u8>>>,
    drained: mpsc::Receiver<()>,
    exited: Arc<AtomicBool>,
    pub answered: u32,
}

impl Process {
    pub fn start(settings: &Settings, started: impl FnOnce(&Arc<Child>) -> io::Result<()>) -> io::Result<Self> {
        let memory = settings.memory.to_string();
        let args = ["work".as_ref(), settings.role.as_ref(), memory.as_ref()];
        let cwd = std::env::current_dir()?;
        let mut child = Child::spawn(&settings.program, &args, &cwd, &std::env::vars_os().collect::<Vec<_>>())?;
        let input = child.stdin.take().expect("worker stdin");
        let output = child.stdout.take().expect("worker stdout");
        let mut stderr = child.stderr.take().expect("worker stderr");
        let child = Arc::new(child);
        // A failed thread start must also terminate a child already started.
        let mut starting = Kill(Some(child.clone()));
        started(&child)?;
        let said = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR)));
        let tail = said.clone();
        let (finished, drained) = mpsc::channel();
        std::thread::Builder::new().name(format!("{} worker stderr", settings.name)).spawn(move || {
            let _role = crate::roles::scoped(crate::roles::Role::Blocking);
            let mut bytes = [0; STDERR];
            loop {
                match stderr.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(length) => {
                        let mut tail = tail.lock().expect("never poisoned");
                        let remove = (tail.len() + length).saturating_sub(STDERR);
                        tail.drain(..remove);
                        tail.extend(&bytes[..length]);
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            let _ = finished.send(());
        })?;
        let exited = Arc::new(AtomicBool::new(false));
        let ended = exited.clone();
        let watching = child.clone();
        std::thread::Builder::new().name(format!("{} worker exit", settings.name)).spawn(move || {
            let _role = crate::roles::scoped(crate::roles::Role::Blocking);
            let _ = watching.wait();
            ended.store(true, Ordering::Release);
            // A descendant may still hold stdout/stderr open after the worker
            // exits. End the group/job before the readers wait for EOF.
            let _ = watching.kill();
        })?;
        starting.0 = None;
        Ok(Self { child, input, output, said, drained, exited, answered: 0 })
    }

    pub fn alive(&self) -> bool {
        !self.exited.load(Ordering::Acquire)
    }

    pub fn exchange(&mut self, request: &[u8]) -> Result<Vec<u8>, Failure> {
        let result = protocol::write(&mut self.input, request).and_then(|()| protocol::answer(&mut self.output));
        match result {
            Ok(Ok(bytes)) => {
                self.answered = self.answered.saturating_add(1);
                Ok(bytes)
            }
            Ok(Err(reason)) => Err(Failure::Refused(reason)),
            Err(_) => {
                // EOF can beat the exit observer. The caller's deadline still
                // bounds this wait, including a worker that closes its pipes.
                let status = self.child.wait().map_err(Failure::Unavailable)?.to_string();
                let _ = self.child.kill();
                let _ = self.drained.recv();
                let bytes: Vec<_> = self.said.lock().expect("never poisoned").iter().copied().collect();
                Err(Failure::Died { status, said: String::from_utf8_lossy(&bytes).into_owned() })
            }
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Kill(Option<Arc<Child>>);

impl Drop for Kill {
    fn drop(&mut self) {
        if let Some(child) = &self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
