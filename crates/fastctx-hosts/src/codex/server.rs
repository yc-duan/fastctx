//! One running app-server and its JSON-RPC client. The transport owns the
//! socket connection and watches the real process. Responses are delivered
//! after preceding notifications; unanswered requests fail after the host
//! has applied the process's exit.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use fastctx_platform::Child;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message;

use super::{Log, transport};

/// What the app-server says that is not the answer to one of our requests.
#[derive(Debug)]
pub enum Inbound {
    /// Complete the RPC only after preceding notifications reached host state.
    Response {
        waiter: oneshot::Sender<Result<Value, Unanswered>>,
        answer: Result<Value, Unanswered>,
    },
    Notification {
        method: String,
        params: Value,
        emitted_at_ms: Option<i64>,
    },
    /// It asks something of the client.
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// It stopped: how it exited, such as `exit code: 1`. Nothing more
    /// comes.
    Exited {
        why: String,
        drained: oneshot::Sender<()>,
    },
}

pub struct AppServer {
    instance: String,
    process: Option<fastctx_types::ProcessIdentity>,
    child: Arc<Child>,
    messages: mpsc::UnboundedSender<Message>,
    ended: watch::Receiver<bool>,
    next: AtomicU64,
    pending: Arc<Mutex<Pending>>,
}

/// Why a request has no answer.
#[derive(Clone, Debug)]
pub enum Unanswered {
    Rpc {
        code: i64,
        message: String,
        data: Value,
    },
    /// It answered with an error, or the request never reached it.
    Refused(String),
    /// It stopped while the request waited: whether it acted on it is not
    /// known.
    Stopped(String),
}

/// How long a request may wait for its answer: greeting and what only reads
/// are quick; what changes a thread or starts something may first wait for
/// the app-server to load what it needs.
fn deadline(method: &str) -> std::time::Duration {
    const READS: &[&str] = &[
        "initialize",
        "thread/read",
        "thread/list",
        "thread/items/list",
        "thread/turns/list",
        "account/read",
        "account/rateLimits/read",
        "account/usage/read",
        "model/list",
        "config/read",
        "skills/list",
    ];
    std::time::Duration::from_secs(if READS.contains(&method) { 60 } else { 120 })
}

impl std::fmt::Display for Unanswered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unanswered::Refused(why) | Unanswered::Stopped(why) => f.write_str(why),
            Unanswered::Rpc { message, .. } => f.write_str(message),
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
        let pending = Arc::new(Mutex::new(Pending::default()));
        let (inbound, receiver) = mpsc::unbounded_channel();
        let (routed, reports, logger) = (pending.clone(), inbound.clone(), log.clone());
        let transport = transport::spawn(program, args, cwd, env, log.clone(), move |bytes| {
            route(bytes, &routed, &reports, &logger);
        })?;
        let (ended, ending) = watch::channel(false);
        let waited = pending.clone();
        tokio::spawn(async move {
            let why = transport.exited.await.unwrap_or_else(|_| "its transport stopped".into());
            let reason = format!("the Codex app-server stopped ({why})");
            let waiting = {
                let mut pending = waited.lock().expect("never poisoned");
                pending.stopped = Some(reason.clone());
                pending.waiting.drain().map(|(_, waiter)| waiter).collect::<Vec<_>>()
            };
            // The host applies all preceding notifications and this exit
            // before deciding whether an unanswered submission went in.
            let (drained, heard) = oneshot::channel();
            let _ = inbound.send(Inbound::Exited { why, drained });
            let _ = heard.await;
            for waiter in waiting {
                let _ = waiter.send(Err(Unanswered::Stopped(reason.clone())));
            }
            let _ = ended.send(true);
        });
        let process = match fastctx_platform::identity_of(transport.child.pid()) {
            Ok(process) => process,
            Err(error) => {
                log(format!("identifying the Codex app-server lifetime: {error}"));
                None
            }
        };
        let server = Arc::new(Self {
            process,
            instance: format!("{}:{}", transport.child.pid(), jiff::Timestamp::now()),
            child: transport.child,
            messages: transport.outgoing,
            ended: ending,
            next: AtomicU64::new(1),
            pending,
        });
        Ok((server, receiver))
    }

    /// Waits for the actual process to end and its events to reach the host.
    pub async fn wait(&self) {
        let mut ended = self.ended.clone();
        let _ = ended.wait_for(|ended| *ended).await;
    }

    pub fn process(&self) -> Option<fastctx_types::ProcessIdentity> {
        self.process.clone()
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
        struct Waiting<'a>(&'a AppServer, u64);
        impl Drop for Waiting<'_> {
            fn drop(&mut self) {
                let mut pending = self.0.pending.lock().expect("never poisoned");
                pending.waiting.remove(&self.1);
                if pending.waiting.is_empty() {
                    pending.waiting = HashMap::new();
                }
            }
        }
        let _waiting = Waiting(self, id);
        if let Err(error) = self.send(&json!({ "id": id, "method": method, "params": params })) {
            self.pending.lock().expect("never poisoned").waiting.remove(&id);
            return Err(Unanswered::Refused(error));
        }
        // Past its deadline, whether it acted is not known, as when it stops
        // (`specs/world-concurrency.md`, 等外面的事都有期限).
        let within = deadline(method);
        let _outside = fastctx_platform::doing::waiting_outside(format!("Codex's {method}"), within);
        match tokio::time::timeout(within, answered).await {
            Ok(answer) => {
                answer.unwrap_or_else(|_| Err(Unanswered::Stopped("the Codex app-server stopped".to_string())))
            }
            Err(_) => Err(Unanswered::Stopped(format!(
                "the Codex app-server did not answer {method} within {} s",
                within.as_secs()
            ))),
        }
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.send(&json!({ "method": method, "params": params }))
    }

    /// Answers one of its requests with an error.
    pub fn refuse(&self, id: Value, code: i64, message: &str) -> Result<(), String> {
        self.send(&json!({ "id": id, "error": { "code": code, "message": message } }))
    }

    /// An opaque reply address distinguishes IDs reused after a restart.
    pub fn request_key(&self, id: &Value) -> Value {
        json!({"server": self.instance, "id": id})
    }

    pub fn answer(&self, id: &Value, answer: &Value) -> Result<(), String> {
        self.send(&json!({"id": id, "result": answer}))
    }

    /// Ends it and everything it started; its exit is reported as usual.
    pub fn kill(&self) {
        let _ = self.child.kill();
    }

    fn send(&self, message: &Value) -> Result<(), String> {
        if let Some(reason) = &self.pending.lock().expect("never poisoned").stopped {
            return Err(reason.clone());
        }
        let text = serde_json::to_string(message).expect("JSON values serialize");
        self.messages.send(Message::Text(text.into())).map_err(|_| "the Codex app-server connection closed".to_string())
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
            let _ = inbound.send(Inbound::Request { id: id.clone(), method, params });
        }
        (Some(method), None) => {
            let _ = inbound.send(Inbound::Notification {
                method,
                params,
                emitted_at_ms: message.get("emittedAtMs").and_then(Value::as_i64),
            });
        }
        (None, Some(id)) => {
            let Some(waiter) = id.as_u64().and_then(|id| pending.lock().expect("never poisoned").waiting.remove(&id))
            else {
                return;
            };
            let answer = match message.get("error") {
                Some(error) => Err(Unanswered::Rpc {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or_default(),
                    data: error.get("data").cloned().unwrap_or(Value::Null),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| error.to_string()),
                }),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = inbound.send(Inbound::Response { waiter, answer });
        }
        (None, None) => {}
    }
}
