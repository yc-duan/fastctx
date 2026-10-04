//! One running `codex app-server` and the client half of its protocol:
//! JSON-RPC messages, one per line, over its standard input and output.
//!
//! The process is the node's own child (`fastctx_platform::Child`): it ends
//! when the node does, and takes the tool servers it started with it. Its
//! end is read from the process, never from its output closing, since a
//! program it started may hold that output open. Once it has exited, what
//! it started is ended too, the last lines it wrote are read, and every
//! request still waiting fails with the reason; nothing is sent to it after.
//!
//! Standard output is read on a thread of its own, as is standard error,
//! which goes to the node's log line by line; writing is a thread too, so
//! a full pipe never holds up the async runtime.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use fastctx_platform::Child;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use super::Log;

/// What the app-server says that is not the answer to one of our requests.
#[derive(Debug)]
pub enum Inbound {
    Notification {
        method: String,
        params: Value,
    },
    /// It asks something of the client.
    Request {
        id: Value,
        method: String,
    },
    /// It stopped: how it exited, such as `exit code: 1`. Nothing more
    /// comes.
    Exited {
        why: String,
        drained: oneshot::Sender<()>,
    },
}

pub struct AppServer {
    child: Arc<Child>,
    lines: std::sync::mpsc::Sender<Vec<u8>>,
    next: AtomicU64,
    pending: Arc<Mutex<Pending>>,
}

/// Why a request has no answer.
#[derive(Clone, Debug)]
pub enum Unanswered {
    /// It answered with an error, or the request never reached it.
    Refused(String),
    /// It stopped while the request waited: whether it acted on it is not
    /// known.
    Stopped(String),
}

impl std::fmt::Display for Unanswered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unanswered::Refused(why) | Unanswered::Stopped(why) => f.write_str(why),
        }
    }
}

#[derive(Default)]
struct Pending {
    waiting: HashMap<u64, oneshot::Sender<Result<Value, Unanswered>>>,
    /// Why it stopped, once it has.
    stopped: Option<String>,
}

impl AppServer {
    /// Starts `program` with `args`; the returned receiver has everything
    /// it says other than answers, ending with `Inbound::Exited`.
    pub fn spawn(
        program: &Path,
        args: &[String],
        cwd: &Path,
        env: &[(OsString, OsString)],
        log: Log,
    ) -> std::io::Result<(Arc<Self>, mpsc::UnboundedReceiver<Inbound>)> {
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        let mut child = Child::spawn(program, &args, cwd, env)?;
        let (stdin, stdout, stderr) = (
            child.stdin.take().expect("piped at spawn"),
            child.stdout.take().expect("piped at spawn"),
            child.stderr.take().expect("piped at spawn"),
        );
        let child = Arc::new(child);
        let pending = Arc::new(Mutex::new(Pending::default()));
        let (inbound, receiver) = mpsc::unbounded_channel();

        let (lines, to_write) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new().name("app-server input".to_string()).spawn(move || {
            let mut stdin = stdin;
            for line in to_write {
                if stdin.write_all(&line).and_then(|()| stdin.flush()).is_err() {
                    // It stopped reading, which is its exit; that is
                    // reported from the process.
                    return;
                }
            }
        })?;

        let stderr_log = log.clone();
        std::thread::Builder::new().name("app-server stderr".to_string()).spawn(move || {
            for line in BufReader::new(stderr).lines() {
                match line {
                    Ok(line) => stderr_log(format!("app-server: {line}")),
                    Err(_) => return,
                }
            }
        })?;

        let reader = {
            let (pending, inbound, log) = (pending.clone(), inbound.clone(), log.clone());
            std::thread::Builder::new().name("app-server output".to_string()).spawn(move || {
                let mut stdout = BufReader::new(stdout);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    match stdout.read_until(b'\n', &mut line) {
                        Ok(0) => return,
                        Ok(_) => route(&line, &pending, &inbound, &log),
                        Err(error) => {
                            log(format!("reading the app-server's output: {error}"));
                            return;
                        }
                    }
                }
            })?
        };

        let (waited, ended) = (child.clone(), pending.clone());
        std::thread::Builder::new().name("app-server exit".to_string()).spawn(move || {
            let pending = ended;
            let status = waited.wait();
            // What it started goes with it, so its output closes and the
            // last of it can be read.
            let _ = waited.kill();
            let _ = reader.join();
            let why = match status {
                Ok(status) => status.to_string(),
                Err(error) => format!("its exit could not be watched: {error}"),
            };
            let reason = format!("the Codex app-server stopped ({why})");
            let mut pending = pending.lock().expect("never poisoned");
            pending.stopped = Some(reason.clone());
            let waiting = pending.waiting.drain().map(|(_, waiter)| waiter).collect::<Vec<_>>();
            drop(pending);
            // Every preceding notification must reach the host state before
            // an unanswered submission is classified as unknown.
            let (drained, heard) = oneshot::channel();
            let _ = inbound.send(Inbound::Exited { why, drained });
            let _ = heard.blocking_recv();
            for waiter in waiting {
                let _ = waiter.send(Err(Unanswered::Stopped(reason.clone())));
            }
        })?;

        let server = Arc::new(Self { child, lines, next: AtomicU64::new(1), pending });
        Ok((server, receiver))
    }

    pub fn pid(&self) -> u32 {
        self.child.pid()
    }

    /// Sends a request and waits for its answer: its `result`, or its
    /// `error`'s message.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        self.ask(method, params).await.map_err(|unanswered| unanswered.to_string())
    }

    /// A request, telling a refusal from a stop that leaves whether it acted
    /// unknown.
    pub async fn ask(&self, method: &str, params: Value) -> Result<Value, Unanswered> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (answer, answered) = oneshot::channel();
        {
            let mut pending = self.pending.lock().expect("never poisoned");
            if let Some(reason) = &pending.stopped {
                return Err(Unanswered::Refused(reason.clone()));
            }
            pending.waiting.insert(id, answer);
        }
        if let Err(error) = self.send(&json!({ "id": id, "method": method, "params": params })) {
            self.pending.lock().expect("never poisoned").waiting.remove(&id);
            return Err(Unanswered::Refused(error));
        }
        answered.await.unwrap_or_else(|_| Err(Unanswered::Stopped("the Codex app-server stopped".to_string())))
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.send(&json!({ "method": method, "params": params }))
    }

    /// Answers one of its requests with an error.
    pub fn refuse(&self, id: Value, code: i64, message: &str) -> Result<(), String> {
        self.send(&json!({ "id": id, "error": { "code": code, "message": message } }))
    }

    /// Ends it and everything it started; its exit is reported as usual.
    pub fn kill(&self) {
        let _ = self.child.kill();
    }

    fn send(&self, message: &Value) -> Result<(), String> {
        if let Some(reason) = &self.pending.lock().expect("never poisoned").stopped {
            return Err(reason.clone());
        }
        let mut line = serde_json::to_vec(message).expect("JSON values serialize");
        line.push(b'\n');
        self.lines.send(line).map_err(|_| "the Codex app-server is not reading its input".to_string())
    }
}

/// Hands one line the app-server wrote to whoever waits for it.
fn route(line: &[u8], pending: &Mutex<Pending>, inbound: &mpsc::UnboundedSender<Inbound>, log: &Log) {
    let text = String::from_utf8_lossy(line);
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let message: Value = match serde_json::from_str(text) {
        Ok(message) => message,
        Err(error) => {
            log(format!("the app-server wrote a line that is not JSON ({error}): {text}"));
            return;
        }
    };
    let method = message.get("method").and_then(Value::as_str).map(str::to_string);
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    match (method, message.get("id")) {
        (Some(method), Some(id)) => {
            let _ = inbound.send(Inbound::Request { id: id.clone(), method });
        }
        (Some(method), None) => {
            let _ = inbound.send(Inbound::Notification { method, params });
        }
        (None, Some(id)) => {
            let Some(waiter) = id.as_u64().and_then(|id| pending.lock().expect("never poisoned").waiting.remove(&id))
            else {
                return;
            };
            let answer = match message.get("error") {
                Some(error) => Err(Unanswered::Refused(
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| error.to_string()),
                )),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = waiter.send(answer);
        }
        (None, None) => {}
    }
}
