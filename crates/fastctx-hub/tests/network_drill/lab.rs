//! What each machine of the drill does with what comes to it. It keeps
//! count of the letters it took, of the times its link connected, and of
//! the calls it is answering, and answers the few calls the scenarios make:
//! an answer that fits one control message, one too large for it, bytes
//! over a data connection, and a question it answers only once given up.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fastctx_net::data::Stream;
use fastctx_net::link::{BoxFuture, Caller, Node};
use fastctx_net::message::LetterId;
use fastctx_types::access::Purpose;
use fastctx_types::file_read::{FileRange, ReadFileRequest};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{Letter, RemoteCall, RemoteResult};
use fastctx_types::update::UpdateCall;

/// Bytes of the answer to `Updates { Look }`: within one control message.
pub const IN_A_MESSAGE: usize = 80_000;
/// Bytes of the answer to `Updates { State }`: too large for one control
/// message, so its caller fetches it over a data connection.
pub const OVER_DATA: usize = 300_000;
/// The bytes of a read that sends nothing and keeps its stream open until
/// the stream ends, as a quiet watch does.
pub const QUIET: u64 = u64::MAX;

#[derive(Clone, Debug)]
pub struct Read {
    pub took: Duration,
    pub sent: u64,
    pub outcome: String,
}

pub struct Lab {
    /// How many times each letter was taken, by its id.
    got: Mutex<HashMap<String, usize>>,
    /// How many times the link connected to the hub.
    connected: AtomicUsize,
    /// Whether `Who` is answered only after ten minutes, as a question its
    /// caller gives up first.
    stalls: AtomicBool,
    /// Calls being answered now, and calls stopped before they were done.
    answering: Arc<AtomicUsize>,
    stopped: Arc<AtomicUsize>,
    /// How each read sent over a data connection ended: how long it took,
    /// and what came of it.
    reads: Arc<Mutex<Vec<Read>>>,
    log: Mutex<std::fs::File>,
}

impl Lab {
    pub fn new(log: &Path) -> std::io::Result<Arc<Self>> {
        Ok(Arc::new(Self {
            got: Mutex::new(HashMap::new()),
            connected: AtomicUsize::new(0),
            stalls: AtomicBool::new(false),
            answering: Arc::default(),
            stopped: Arc::default(),
            reads: Arc::default(),
            log: Mutex::new(std::fs::File::create(log)?),
        }))
    }

    /// The letters taken, and how many times each.
    pub fn got(&self) -> HashMap<String, usize> {
        self.got.lock().expect("never poisoned").clone()
    }

    /// How many letters were taken, each counted once.
    pub fn received(&self) -> usize {
        self.got.lock().expect("never poisoned").len()
    }

    pub fn connections(&self) -> usize {
        self.connected.load(Ordering::SeqCst)
    }

    pub fn stall(&self, stalls: bool) {
        self.stalls.store(stalls, Ordering::SeqCst);
    }

    pub fn answering(&self) -> usize {
        self.answering.load(Ordering::SeqCst)
    }

    pub fn stopped(&self) -> usize {
        self.stopped.load(Ordering::SeqCst)
    }

    /// How the reads sent so far ended.
    pub fn reads(&self) -> Vec<Read> {
        self.reads.lock().expect("never poisoned").clone()
    }
}

/// The call `ReadFile` the drill makes for `bytes` sent back over its data
/// connection.
pub fn read_of(bytes: u64) -> RemoteCall {
    RemoteCall::ReadFile {
        request: ReadFileRequest { path: "drill".to_string(), range: FileRange::Range { offset: 0, bytes } },
        read: 0,
    }
}

/// Counts a call while it is answered, and whether it was stopped.
struct Answering {
    answering: Arc<AtomicUsize>,
    stopped: Arc<AtomicUsize>,
    done: bool,
}

impl Answering {
    /// The call was answered, not stopped. Taking the guard whole keeps it
    /// in the future that answers, not only its flag.
    fn answered(mut self) {
        self.done = true;
    }
}

impl Drop for Answering {
    fn drop(&mut self) {
        self.answering.fetch_sub(1, Ordering::SeqCst);
        if !self.done {
            self.stopped.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl Node for Lab {
    fn answering(&self, _: &RemoteCall) -> Result<Option<Box<dyn Send>>, Failure> {
        Ok(None)
    }

    fn call(&self, _: Caller, call: RemoteCall, stream: Option<Stream>) -> BoxFuture<RemoteResult> {
        match call {
            RemoteCall::Updates { update: UpdateCall::Look } => {
                Box::pin(async { Err(Failure::new("x".repeat(IN_A_MESSAGE))) })
            }
            RemoteCall::Updates { update: UpdateCall::State } => {
                Box::pin(async { Err(Failure::new("y".repeat(OVER_DATA))) })
            }
            RemoteCall::ReadFile { request: ReadFileRequest { range: FileRange::Range { bytes, .. }, .. }, .. } => {
                self.answering.fetch_add(1, Ordering::SeqCst);
                let answering =
                    Answering { answering: self.answering.clone(), stopped: self.stopped.clone(), done: false };
                let reads = self.reads.clone();
                Box::pin(async move {
                    let began = std::time::Instant::now();
                    let Some(mut stream) = stream else {
                        return Err(Failure::new("the read came without its data connection"));
                    };
                    let mut sent = 0_u64;
                    let outcome = if bytes == QUIET {
                        stream.ended().await.unwrap_or_else(|| "the stream closed normally".to_string())
                    } else {
                        let buffer = vec![7_u8; 64 * 1024];
                        let transfer = async {
                            while sent < bytes {
                                let count = (bytes - sent).min(buffer.len() as u64) as usize;
                                stream.send(&buffer[..count]).await?;
                                sent += count as u64;
                            }
                            stream.finish().await
                        }
                        .await;
                        transfer.err().unwrap_or_else(|| format!("sent {bytes}"))
                    };
                    reads.lock().expect("never poisoned").push(Read {
                        took: began.elapsed(),
                        sent,
                        outcome: outcome.clone(),
                    });
                    answering.answered();
                    Err(Failure::new(outcome))
                })
            }
            RemoteCall::Who { .. } if self.stalls.load(Ordering::SeqCst) => {
                self.answering.fetch_add(1, Ordering::SeqCst);
                let answering =
                    Answering { answering: self.answering.clone(), stopped: self.stopped.clone(), done: false };
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_secs(600)).await;
                    answering.answered();
                    Err(Failure::new("slept"))
                })
            }
            _ => Box::pin(async { Err(Failure::new("no calls here")) }),
        }
    }

    fn letter(&self, _: Caller, _: Purpose, id: LetterId, _: Letter) -> BoxFuture<std::io::Result<Result<(), String>>> {
        *self.got.lock().expect("never poisoned").entry(id.as_str().to_string()).or_default() += 1;
        Box::pin(async { Ok(Ok(())) })
    }

    fn ledger_changed(&self, _: Option<fastctx_types::access::TownId>) {}

    fn connected(&self) {
        self.connected.fetch_add(1, Ordering::SeqCst);
    }

    fn forget(&self, _: fastctx_net::wire::Forgetting) -> BoxFuture<fastctx_net::wire::Forgot> {
        Box::pin(async { fastctx_net::wire::Forgot::Done })
    }

    fn log(&self, line: String) {
        if let Err(error) = writeln!(self.log.lock().expect("never poisoned"), "{} {line}", jiff::Timestamp::now()) {
            crate::process::failed(error);
        }
    }
}
