//! Serialized background reads for the dashboard; terminal input never waits for registry I/O.

use crate::control::paths::ControlPaths;
use crate::shell::jobs::{self, JobRegistryError, JobSummary, JobTail};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

pub(super) enum Request {
    List {
        observed: Vec<String>,
    },
    Count,
    Tail {
        job_id: String,
        tail: Box<JobTail>,
        max_lines: usize,
        max_line_width: usize,
    },
}

pub(super) enum Reply {
    List(Result<Vec<JobSummary>, JobRegistryError>),
    Count(Option<usize>),
    Tail {
        job_id: String,
        tail: Box<JobTail>,
        result: Result<usize, String>,
        max_line_width: usize,
    },
    Failed(String),
}

struct Channel {
    requests: Sender<Request>,
    replies: Receiver<Reply>,
}

/// Owns one reader thread and admits at most one outstanding request.
#[derive(Default)]
pub(super) struct JobReader {
    channel: Option<Channel>,
    busy: bool,
}

impl JobReader {
    pub(super) fn is_busy(&self) -> bool {
        self.busy
    }

    pub(super) fn start(&mut self, paths: &ControlPaths, request: Request) -> Result<(), String> {
        if self.busy {
            return Err("A dashboard read is already in progress.".into());
        }
        if self.channel.is_none() {
            let paths = paths.clone();
            self.channel = Some(spawn_reader(move |request| read(&paths, request))?);
        }
        if self
            .channel
            .as_ref()
            .expect("the reader was initialized")
            .requests
            .send(request)
            .is_err()
        {
            self.channel = None;
            return Err("The dashboard reader disconnected.".into());
        }
        self.busy = true;
        Ok(())
    }

    pub(super) fn poll(&mut self) -> Option<Reply> {
        if !self.busy {
            return None;
        }
        let reply = match self.channel.as_ref()?.replies.try_recv() {
            Ok(reply) => reply,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                self.channel = None;
                Reply::Failed("The dashboard reader disconnected.".into())
            }
        };
        self.busy = false;
        Some(reply)
    }

    #[cfg(test)]
    pub(super) fn with_reader(read: impl FnMut(Request) -> Reply + Send + 'static) -> Self {
        Self {
            channel: Some(spawn_reader(read).unwrap()),
            busy: false,
        }
    }
}

fn spawn_reader(
    mut read: impl FnMut(Request) -> Reply + Send + 'static,
) -> Result<Channel, String> {
    let (requests, incoming) = mpsc::channel();
    let (outgoing, replies) = mpsc::channel();
    std::thread::Builder::new()
        .name("fastctx-dashboard-reader".into())
        .spawn(move || {
            while let Ok(request) = incoming.recv() {
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read(request)));
                let reply = result.unwrap_or_else(|_| {
                    Reply::Failed("The dashboard reader stopped unexpectedly.".into())
                });
                if outgoing.send(reply).is_err() {
                    break;
                }
            }
        })
        .map_err(|error| format!("Cannot start the dashboard reader: {error}"))?;
    Ok(Channel { requests, replies })
}

fn read(paths: &ControlPaths, request: Request) -> Reply {
    match request {
        Request::List { observed } => Reply::List(jobs::dashboard_summaries(paths, &observed)),
        Request::Count => Reply::Count(jobs::running_count(paths).ok()),
        Request::Tail {
            job_id,
            mut tail,
            max_lines,
            max_line_width,
        } => {
            let previous = std::sync::Arc::clone(&tail.lines);
            let result = jobs::refresh_tail(paths, &job_id, max_lines, &mut tail);
            let max_line_width = if std::sync::Arc::ptr_eq(&previous, &tail.lines) {
                max_line_width
            } else {
                super::jobs::output_width(&tail)
            };
            Reply::Tail {
                job_id,
                tail,
                result,
                max_line_width,
            }
        }
    }
}
