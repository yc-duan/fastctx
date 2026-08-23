//! One MCP session, and the engine links that come and go underneath it.
//!
//! The session is the thing the host owns: it starts when the host spawns `fastctx serve` and ends
//! when the host stops needing it. The engine carrying that session — a shared control center, or
//! a control-center session running inside this process — is replaceable, because a host never
//! recovers from a stdio MCP server that exits. Codex does not restart one, so a session that
//! ended itself over a failed engine link costs that conversation every FastCtx tool until the
//! user notices and starts a new one. Nothing below ends the session on an engine failure.

use super::journal::SessionJournal;
use super::local_ipc::BoxedStream;
use super::{
    RESPONSE_DRAIN_TIMEOUT, STARTUP_TIMEOUT, connect_or_start, protocol, start_in_process,
};
use crate::process_identity::ProcessIdentity;
use crate::server::ServerOptions;
use crate::session::SessionEnvironment;
use crate::stdio_transport::DetachedStdin;
use std::future::Future;
use std::pin::Pin;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf, split};

/// Gap between engine acquisition rounds once both the shared control center and the in-process
/// engine have failed. Reaching a second round means the machine cannot host either, which no
/// ordinary failure produces.
const RELINK_RETRY_GAP: Duration = Duration::from_secs(1);
/// How many retry rounds pass between repeats of the same failure on the host's log.
const RELINK_REPORT_INTERVAL: u32 = 60;
/// Bytes accepted while reading the answer to a replayed `initialize`. A handshake answer is a few
/// kilobytes; the bound only stops a broken engine from streaming forever into the replay.
const REPLAY_ANSWER_LIMIT_BYTES: usize = 1024 * 1024;
const ANSWER_CHUNK_BYTES: usize = 64 * 1024;

/// Runs the whole stdio MCP session: acquire an engine, pump bytes, replace the engine on failure.
pub(crate) async fn run_proxy_session(
    options: ServerOptions,
    environment: SessionEnvironment,
    parent: Option<Option<ProcessIdentity>>,
) -> Result<ExitCode, String> {
    let host = parent.clone().flatten();
    // The first engine is acquired before stdin is touched, so a control center that cannot serve
    // this session degrades without ever consuming an MCP request.
    let stream = match connect_or_start(options, &environment, host.clone()).await {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!(
                "fastctx: control center unavailable ({error}); falling back to a full standalone MCP server."
            );
            start_in_process(options, &environment, host.clone()).await?
        }
    };
    let session = ProxySession {
        stdin: DetachedStdin::start()?,
        stdout: tokio::io::stdout(),
        journal: SessionJournal::new(),
        options,
        environment,
        host,
        input_closed: false,
        resynchronising: false,
    };
    session.run(EngineLink::new(stream), parent).await
}

/// The engine currently carrying the session, split so both directions can be awaited at once.
struct EngineLink {
    reader: ReadHalf<BoxedStream>,
    writer: WriteHalf<BoxedStream>,
}

impl EngineLink {
    fn new(stream: BoxedStream) -> Self {
        let (reader, writer) = split(stream);
        Self { reader, writer }
    }
}

/// Why the byte pump stopped.
enum LinkOutcome {
    /// The session itself is over and the process should exit.
    SessionEnded(Result<(), String>),
    /// The engine link failed while the session is still live.
    LinkLost(String),
}

/// The result of replacing a failed engine link.
enum Recovery {
    Linked(EngineLink),
    SessionEnded(Result<(), String>),
}

/// Everything that outlives an individual engine link.
struct ProxySession {
    stdin: DetachedStdin,
    stdout: tokio::io::Stdout,
    journal: SessionJournal,
    options: ServerOptions,
    environment: SessionEnvironment,
    host: Option<ProcessIdentity>,
    input_closed: bool,
    /// Set after an in-progress request grew past what can be replayed. Request bytes are then
    /// discarded until the next newline, so a replacement engine never receives a spliced message.
    resynchronising: bool,
}

impl ProxySession {
    async fn run(
        mut self,
        mut link: EngineLink,
        parent: Option<Option<ProcessIdentity>>,
    ) -> Result<ExitCode, String> {
        let mut signals = SessionSignals::new(parent, &self.stdin);
        let result = loop {
            match self.pump(&mut link, &mut signals).await {
                LinkOutcome::SessionEnded(result) => break result,
                LinkOutcome::LinkLost(reason) => match self.recover(reason, &mut signals).await {
                    Recovery::Linked(replacement) => link = replacement,
                    Recovery::SessionEnded(result) => break result,
                },
            }
        };
        signals.stop().await;
        result?;
        Ok(ExitCode::SUCCESS)
    }

    /// Moves bytes between the host and one engine link until either the session or the link ends.
    async fn pump(&mut self, link: &mut EngineLink, signals: &mut SessionSignals) -> LinkOutcome {
        let mut request_buffer = vec![0_u8; protocol::MAX_REQUEST_FRAME_BYTES];
        let mut answer_buffer = vec![0_u8; ANSWER_CHUNK_BYTES];
        let mut drain_deadline: Option<tokio::time::Instant> = None;
        loop {
            let Self {
                stdin,
                stdout,
                journal,
                input_closed,
                resynchronising,
                ..
            } = &mut *self;
            tokio::select! {
                biased;
                error = &mut signals.stdin_error => {
                    return LinkOutcome::SessionEnded(Err(error));
                }
                () = &mut signals.parent_exit => return LinkOutcome::SessionEnded(Ok(())),
                () = &mut signals.termination => return LinkOutcome::SessionEnded(Ok(())),
                () = drain_backstop(drain_deadline) => return LinkOutcome::SessionEnded(Ok(())),
                read = stdin.read(&mut request_buffer), if !*input_closed => match read {
                    Err(error) => {
                        return LinkOutcome::SessionEnded(
                            Err(format!("Cannot read MCP stdin: {error}")),
                        );
                    }
                    Ok(0) => {
                        *input_closed = true;
                        if let Err(error) = protocol::write_end_of_input(&mut link.writer).await {
                            return LinkOutcome::LinkLost(format!(
                                "The FastCtx engine connection failed while closing the session: {error}"
                            ));
                        }
                        // Nothing was ever asked, so nothing can be owed; waiting would only stall
                        // a client that opened the transport and changed its mind.
                        if !journal.has_forwarded() {
                            return LinkOutcome::SessionEnded(Ok(()));
                        }
                        drain_deadline =
                            Some(tokio::time::Instant::now() + RESPONSE_DRAIN_TIMEOUT);
                    }
                    Ok(read) => {
                        let mut chunk = &request_buffer[..read];
                        if *resynchronising {
                            match chunk.iter().position(|&byte| byte == b'\n') {
                                Some(index) => {
                                    *resynchronising = false;
                                    chunk = &chunk[index + 1..];
                                }
                                None => continue,
                            }
                        }
                        if chunk.is_empty() {
                            continue;
                        }
                        journal.observe_request_bytes(chunk);
                        match protocol::write_request_frame(&mut link.writer, chunk).await {
                            Ok(()) => journal.mark_delivered(),
                            Err(failure) => {
                                if failure.reached_engine {
                                    journal.mark_delivered();
                                }
                                return LinkOutcome::LinkLost(format!(
                                    "The FastCtx engine stopped accepting requests: {}",
                                    failure.error
                                ));
                            }
                        }
                    }
                },
                read = link.reader.read(&mut answer_buffer) => match read {
                    Ok(0) => {
                        return LinkOutcome::LinkLost(
                            "The FastCtx engine connection closed".to_string(),
                        );
                    }
                    Err(error) => {
                        return LinkOutcome::LinkLost(format!(
                            "The FastCtx engine connection failed: {error}"
                        ));
                    }
                    Ok(read) => {
                        let chunk = &answer_buffer[..read];
                        journal.observe_answer_bytes(chunk);
                        if let Err(error) = write_out(stdout, chunk).await {
                            return LinkOutcome::SessionEnded(Err(error));
                        }
                    }
                },
            }
        }
    }

    /// Closes out the calls a failed engine can no longer answer, then links a replacement.
    async fn recover(&mut self, reason: String, signals: &mut SessionSignals) -> Recovery {
        if self.input_closed {
            // After the host closed stdin, an engine that stops answering is the end of a session
            // that was already ending. There is nothing left to reconnect for.
            let _ = self.close_out_unanswered().await;
            return Recovery::SessionEnded(Ok(()));
        }
        eprintln!("fastctx: {reason}. Reconnecting; this MCP session stays open.");
        if let Err(error) = self.close_out_unanswered().await {
            return Recovery::SessionEnded(Err(error));
        }
        // No engine can take over a session whose opening handshake was never retained, so this
        // ends the session honestly instead of retrying something that can only fail again.
        if let Some(blocked) = self.journal.resume_blocked() {
            return Recovery::SessionEnded(Err(blocked));
        }
        let mut attempts: u32 = 0;
        loop {
            // The first failure of a round explains the situation; repeating it every second
            // would only bury the host's log while the retry loop does its work.
            let report = attempts.is_multiple_of(RELINK_REPORT_INTERVAL);
            tokio::select! {
                biased;
                error = &mut signals.stdin_error => {
                    return Recovery::SessionEnded(Err(error));
                }
                () = &mut signals.parent_exit => return Recovery::SessionEnded(Ok(())),
                () = &mut signals.termination => return Recovery::SessionEnded(Ok(())),
                acquired = self.acquire_engine(report) => match acquired {
                    Ok(link) => {
                        eprintln!("fastctx: reconnected; FastCtx tools are available again.");
                        return Recovery::Linked(link);
                    }
                    Err(error) => {
                        if report {
                            eprintln!("fastctx: {error} Retrying every second.");
                        }
                        attempts = attempts.saturating_add(1);
                    }
                },
            }
            tokio::select! {
                biased;
                error = &mut signals.stdin_error => {
                    return Recovery::SessionEnded(Err(error));
                }
                () = &mut signals.parent_exit => return Recovery::SessionEnded(Ok(())),
                () = &mut signals.termination => return Recovery::SessionEnded(Ok(())),
                () = tokio::time::sleep(RELINK_RETRY_GAP) => {}
            }
        }
    }

    /// Prefers the shared control center and falls back to an engine inside this process.
    ///
    /// `report` gates the explanation of why the shared runtime was skipped, so a retry loop does
    /// not repeat it once a second.
    async fn acquire_engine(&mut self, report: bool) -> Result<EngineLink, String> {
        let stream = match connect_or_start(self.options, &self.environment, self.host.clone())
            .await
        {
            Ok(stream) => stream,
            Err(shared) => {
                if report {
                    eprintln!(
                        "fastctx: {shared} Running the FastCtx engine inside this session instead."
                    );
                }
                start_in_process(self.options, &self.environment, self.host.clone())
                    .await
                    .map_err(|error| {
                        format!("Cannot start an in-process FastCtx engine either: {error}")
                    })?
            }
        };
        let mut link = EngineLink::new(stream);
        self.resume_on(&mut link).await?;
        Ok(link)
    }

    /// Tells a replacement engine everything it needs to carry the session already in progress.
    async fn resume_on(&mut self, link: &mut EngineLink) -> Result<(), String> {
        let plan = self.journal.resume_plan()?;
        let Some(initialize) = plan.initialize else {
            // The engine failed before the host initialised the session; its `initialize` is still
            // on the way and needs no replay.
            return self.replay_partial(link, plan.partial_request).await;
        };
        protocol::write_request_frame(&mut link.writer, &initialize)
            .await
            .map_err(|failure| {
                format!(
                    "Cannot replay the MCP handshake onto a new engine: {}",
                    failure.error
                )
            })?;
        let answer = tokio::time::timeout(STARTUP_TIMEOUT, read_answer_line(&mut link.reader))
            .await
            .map_err(|_| {
                "Timed out replaying the MCP handshake onto a new FastCtx engine.".to_string()
            })??;
        // The host already holds an answer to this request unless the engine died before
        // delivering it, in which case the replayed answer is the one it has been waiting for.
        if plan.initialize_owed {
            write_out(&mut self.stdout, &answer).await?;
        }
        self.journal.accept_replayed_initialize();
        if let Some(initialized) = plan.initialized {
            protocol::write_request_frame(&mut link.writer, &initialized)
                .await
                .map_err(|failure| {
                    format!(
                        "Cannot replay the MCP handshake onto a new engine: {}",
                        failure.error
                    )
                })?;
        }
        self.replay_partial(link, plan.partial_request).await
    }

    async fn replay_partial(
        &mut self,
        link: &mut EngineLink,
        partial: Option<Vec<u8>>,
    ) -> Result<(), String> {
        match partial {
            Some(bytes) if !bytes.is_empty() => {
                protocol::write_request_frame(&mut link.writer, &bytes)
                    .await
                    .map_err(|failure| {
                        format!(
                            "Cannot replay a request in progress onto a new engine: {}",
                            failure.error
                        )
                    })
            }
            Some(_) => Ok(()),
            None => {
                self.journal.abandon_partial_request();
                self.resynchronising = true;
                Ok(())
            }
        }
    }

    /// Answers every call the failed engine left hanging, so the model is told rather than left
    /// waiting for a result that can no longer arrive.
    async fn close_out_unanswered(&mut self) -> Result<(), String> {
        if !self.journal.tracking_is_complete() {
            eprintln!(
                "fastctx: this session had more calls in flight than FastCtx tracks, so some of them cannot be closed out and will time out on the host instead."
            );
        }
        let unanswered = self.journal.take_unanswered();
        if unanswered.is_empty() {
            return Ok(());
        }
        for request in &unanswered {
            write_out(&mut self.stdout, &request.failure_line()).await?;
        }
        Ok(())
    }
}

/// Writes one message to the host and flushes it, because a buffered answer is an unanswered one.
async fn write_out(stdout: &mut tokio::io::Stdout, bytes: &[u8]) -> Result<(), String> {
    stdout
        .write_all(bytes)
        .await
        .map_err(|error| format!("Cannot write the MCP answer stream: {error}"))?;
    stdout
        .flush()
        .await
        .map_err(|error| format!("Cannot flush the MCP answer stream: {error}"))
}

/// Reads one newline-delimited message. A raw newline never appears inside a JSON string, so the
/// first one always ends the message.
async fn read_answer_line(reader: &mut ReadHalf<BoxedStream>) -> Result<Vec<u8>, String> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        match reader.read(&mut byte).await {
            Ok(0) => {
                return Err(
                    "The new FastCtx engine closed before answering the replayed MCP handshake."
                        .to_string(),
                );
            }
            Ok(_) => {
                line.push(byte[0]);
                if byte[0] == b'\n' {
                    return Ok(line);
                }
                if line.len() > REPLAY_ANSWER_LIMIT_BYTES {
                    return Err(
                        "The new FastCtx engine answered the replayed MCP handshake with an unbounded message."
                            .to_string(),
                    );
                }
            }
            Err(error) => {
                return Err(format!(
                    "Cannot read the replayed MCP handshake answer: {error}"
                ));
            }
        }
    }
}

/// Bounds how long a closing session waits for answers the engine still owes.
async fn drain_backstop(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// The events that end a session no matter which engine is carrying it.
struct SessionSignals {
    parent_exit: Pin<Box<dyn Future<Output = ()> + Send>>,
    termination: Pin<Box<dyn Future<Output = ()> + Send>>,
    stdin_error: Pin<Box<dyn Future<Output = String> + Send>>,
    monitor: Option<tokio::task::JoinHandle<()>>,
    monitor_stop: Arc<AtomicBool>,
}

impl SessionSignals {
    fn new(parent: Option<Option<ProcessIdentity>>, stdin: &DetachedStdin) -> Self {
        let monitor_stop = Arc::new(AtomicBool::new(false));
        let (parent_exit, monitor) = parent_exit_monitor(parent, Arc::clone(&monitor_stop));
        Self {
            parent_exit,
            termination: Box::pin(wait_for_termination_signal()),
            stdin_error: Box::pin(wait_for_stdin_error(stdin.read_error_receiver())),
            monitor,
            monitor_stop,
        }
    }

    async fn stop(self) {
        self.monitor_stop.store(true, Ordering::Release);
        if let Some(monitor) = self.monitor {
            let _ = monitor.await;
        }
    }
}

type ParentExitFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

fn parent_exit_monitor(
    parent: Option<Option<ProcessIdentity>>,
    stop: Arc<AtomicBool>,
) -> (ParentExitFuture, Option<tokio::task::JoinHandle<()>>) {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let monitor = match parent {
        None => None,
        Some(None) => {
            let _ = sender.send(());
            return (Box::pin(async {}), None);
        }
        Some(Some(identity)) => Some(tokio::task::spawn_blocking(move || {
            if crate::process_identity::wait_for_identity_exit_until(&identity, &stop) {
                let _ = sender.send(());
            }
        })),
    };
    let future = async move {
        match receiver.await {
            Ok(()) => {}
            // Monitor failure is not proof that the host exited. Keep the session alive until an
            // explicit end instead of killing a live conversation.
            Err(_) => std::future::pending::<()>().await,
        }
    };
    (Box::pin(future), monitor)
}

async fn wait_for_stdin_error(
    mut receiver: tokio::sync::watch::Receiver<Option<String>>,
) -> String {
    loop {
        if let Some(error) = receiver.borrow().clone() {
            return error;
        }
        if receiver.changed().await.is_err() {
            return std::future::pending::<String>().await;
        }
    }
}

#[cfg(unix)]
async fn wait_for_termination_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let Ok(mut terminate) = signal(SignalKind::terminate()) else {
        return std::future::pending::<()>().await;
    };
    let Ok(mut interrupt) = signal(SignalKind::interrupt()) else {
        return std::future::pending::<()>().await;
    };
    tokio::select! {
        _ = terminate.recv() => {}
        _ = interrupt.recv() => {}
    }
}

#[cfg(not(unix))]
async fn wait_for_termination_signal() {
    std::future::pending::<()>().await
}
