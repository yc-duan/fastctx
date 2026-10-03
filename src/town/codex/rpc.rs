//! JSON-RPC over the stdio of an unmodified `codex app-server`.
//!
//! The app-server speaks newline-delimited JSON: requests and responses correlated by id,
//! notifications from the server, and requests the server sends to the client (approvals,
//! elicitations). A request never times out here: the server either answers or exits, and its
//! exit fails every pending request with the reason, so nothing waits forever and nothing is
//! cut off by a guess.

use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};

/// Something the app-server sent that is not an answer to one of our requests.
#[derive(Debug)]
pub(crate) enum Inbound {
    Notification {
        method: String,
        params: Value,
    },
    /// The server asks the client something and waits for `respond`.
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// The process is gone; every pending request has already failed.
    Exited(String),
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// One running app-server and the client half of its protocol.
pub(crate) struct AppServer {
    stdin: tokio::sync::Mutex<ChildStdin>,
    next_id: AtomicU64,
    pending: Pending,
    child: Mutex<Option<Child>>,
    pid: Option<u32>,
}

impl AppServer {
    /// Starts `binary args…` and returns the connection with the stream of everything the
    /// server sends unprompted. `log` receives the server's stderr lines.
    pub(crate) fn spawn(
        binary: &Path,
        args: &[String],
        cwd: &Path,
        log: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Result<(Arc<Self>, mpsc::UnboundedReceiver<Inbound>), String> {
        let mut command = Command::new(binary);
        command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            // CREATE_NO_WINDOW: the daemon is headless and the app-server must not flash a console.
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|error| {
            format!(
                "Cannot start the Codex app-server {}: {error}",
                crate::paths::display_path(binary)
            )
        })?;
        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (sender, receiver) = mpsc::unbounded_channel();
        let server = Arc::new(Self {
            stdin: tokio::sync::Mutex::new(stdin),
            next_id: AtomicU64::new(1),
            pending: Arc::clone(&pending),
            pid: child.id(),
            child: Mutex::new(Some(child)),
        });

        let stderr_log = Arc::clone(&log);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                stderr_log(format!("app-server stderr: {line}"));
            }
        });
        tokio::spawn(read_stdout(stdout, pending, sender, log));
        Ok((server, receiver))
    }

    pub(crate) fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Sends a request and waits for its answer or the server's exit.
    pub(crate) async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().insert(id, sender);
        if let Err(error) = self
            .write(&json!({"id": id, "method": method, "params": params}))
            .await
        {
            self.pending.lock().remove(&id);
            return Err(error);
        }
        receiver
            .await
            .map_err(|_| format!("{method}: the Codex app-server went away before answering."))?
            .map_err(|error| format!("{method}: {error}"))
    }

    pub(crate) async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.write(&json!({"method": method, "params": params}))
            .await
    }

    /// Answers a request the server sent.
    pub(crate) async fn respond(
        &self,
        id: Value,
        outcome: Result<Value, (i64, String)>,
    ) -> Result<(), String> {
        let message = match outcome {
            Ok(result) => json!({"id": id, "result": result}),
            Err((code, message)) => json!({"id": id, "error": {"code": code, "message": message}}),
        };
        self.write(&message).await
    }

    /// Stops the process. Pending requests fail when its stdout closes.
    pub(crate) fn kill(&self) {
        if let Some(mut child) = self.child.lock().take() {
            let _ = child.start_kill();
        }
    }

    async fn write(&self, message: &Value) -> Result<(), String> {
        let mut line = serde_json::to_vec(message)
            .map_err(|error| format!("Cannot encode an app-server message: {error}"))?;
        line.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(&line)
            .await
            .and(stdin.flush().await)
            .map_err(|error| format!("Cannot write to the Codex app-server: {error}"))
    }
}

async fn read_stdout(
    stdout: tokio::process::ChildStdout,
    pending: Pending,
    sender: mpsc::UnboundedSender<Inbound>,
    log: Arc<dyn Fn(String) + Send + Sync>,
) {
    let mut lines = BufReader::new(stdout).lines();
    let reason = loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(&line) {
                    Ok(message) => route(message, &pending, &sender),
                    Err(error) => log(format!(
                        "app-server wrote a line that is not JSON ({error}): {line}"
                    )),
                }
            }
            Ok(None) => break "its output closed".to_string(),
            Err(error) => break format!("its output could not be read ({error})"),
        }
    };
    let reason = format!("The Codex app-server stopped: {reason}.");
    for (_, waiter) in pending.lock().drain() {
        let _ = waiter.send(Err(reason.clone()));
    }
    let _ = sender.send(Inbound::Exited(reason));
}

fn route(message: Value, pending: &Pending, sender: &mpsc::UnboundedSender<Inbound>) {
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string);
    let id = message.get("id").cloned();
    match (method, id) {
        (Some(method), Some(id)) => {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            let _ = sender.send(Inbound::Request { id, method, params });
        }
        (Some(method), None) => {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            let _ = sender.send(Inbound::Notification { method, params });
        }
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else { return };
            let Some(waiter) = pending.lock().remove(&id) else {
                return;
            };
            let outcome = match message.get("error") {
                Some(error) => Err(error
                    .get("message")
                    .and_then(Value::as_str)
                    .map_or_else(|| error.to_string(), str::to_string)),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = waiter.send(outcome);
        }
        (None, None) => {}
    }
}
