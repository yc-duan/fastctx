//! WebSocket over a Unix socket, with the server's own process as
//! the authority for its lifetime. Losing our connection ends that process.

use std::ffi::{OsStr, OsString};
use std::io::{self, BufRead as _, BufReader};
use std::path::Path;
use std::sync::Arc;

use fastctx_platform::Child;
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::{
    Message,
    protocol::{
        WebSocketConfig,
        frame::{
            Frame,
            coding::{Data, OpCode},
        },
    },
};

use super::Log;

/// How long the app-server may take to listen, and then to take the node's
/// connection (`specs/world-concurrency.md`, 等外面的事都有期限).
const STARTING: std::time::Duration = std::time::Duration::from_secs(60);

const LISTENER_TARGET: &str = "codex_app_server_transport::transport::unix_socket";
const LISTENING: &str = "app-server control socket listening";

pub struct Transport {
    pub child: Arc<Child>,
    pub outgoing: mpsc::UnboundedSender<Message>,
    /// Sent only after the process ended and the connection finished routing.
    pub exited: oneshot::Receiver<String>,
}

pub fn spawn(
    program: &Path,
    args: &[String],
    cwd: &Path,
    env: &[(OsString, OsString)],
    log: Log,
    route: impl Fn(&[u8]) + Send + Sync + 'static,
) -> io::Result<Transport> {
    fastctx_platform::private::ensure_user_only_dir(cwd)?;
    let socket = std::path::absolute(cwd)?.join("app-server.sock");
    let address = format!("unix://{}", socket.to_str().ok_or_else(|| io::Error::other("socket path is not UTF-8"))?);
    let mut args: Vec<OsString> = args.iter().map(OsString::from).collect();
    args.extend(["--listen".into(), address.into()]);
    let mut server_env = env.to_vec();
    let filter = server_env.iter().find(|(name, _)| name.eq_ignore_ascii_case("RUST_LOG"));
    let mut filter = filter.map(|(_, value)| value.clone()).unwrap_or_default();
    if !filter.is_empty() {
        filter.push(",");
    }
    // Codex 0.160 emits this event after bind, permissions and publication.
    // Waiting for it avoids connecting to a merely-created socket file.
    filter.push(format!("{LISTENER_TARGET}=info"));
    server_env.retain(|(name, _)| !name.eq_ignore_ascii_case("RUST_LOG") && !name.eq_ignore_ascii_case("LOG_FORMAT"));
    server_env.extend([("RUST_LOG".into(), filter), ("LOG_FORMAT".into(), "json".into())]);
    let args: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
    let mut child = Child::spawn(program, &args, cwd, &server_env)?;
    drop(child.stdin.take());
    let stdout = child.stdout.take().expect("piped at spawn");
    let stderr = child.stderr.take().expect("piped at spawn");
    let child = Arc::new(child);
    let (ready, listening) = oneshot::channel();
    let (stopped, stopping) = watch::channel(false);
    let (exit, exited) = oneshot::channel();

    let started = (|| {
        read_log(stdout, "app-server stdout", log.clone(), None)?;
        read_log(stderr, "app-server", log.clone(), Some(ready))?;
        let (child, log) = (child.clone(), log.clone());
        std::thread::Builder::new().name("app-server exit".into()).spawn(move || {
            let status = child.wait();
            kill(&child, "app-server", &log);
            let _ = stopped.send(true);
            let why = match status {
                Ok(status) => status.to_string(),
                Err(error) => format!("its exit could not be watched: {error}"),
            };
            let _ = exit.send(why);
        })?;
        Ok::<_, io::Error>(())
    })();
    if let Err(error) = started {
        kill(&child, "app-server", &log);
        return Err(error);
    }

    let (outgoing, messages) = mpsc::unbounded_channel();
    let (finished, drained) = oneshot::channel();
    let held = child.clone();
    let replies = outgoing.clone();
    tokio::spawn(async move {
        let run = async {
            let mut stopping = stopping;
            tokio::select! {
                ready = tokio::time::timeout(STARTING, listening) => match ready {
                    Ok(ready) => ready.map_err(|_| "app-server stderr closed before its socket was listening".to_string())?,
                    Err(_) => return Err(format!("the app-server did not listen within {} s", STARTING.as_secs())),
                },
                _ = stopping.wait_for(|stopped| *stopped) => return Ok(()),
            }
            let connect = async {
                let stream = fastctx_platform::unix_socket::connect(&socket).await
                    .map_err(|error| format!("connecting socket: {error}"))?;
                // The server advertises its incoming frame limit. History and
                // media must not acquire a separate client-side size cap.
                let config = WebSocketConfig::default().max_message_size(None).max_frame_size(None);
                tokio_tungstenite::client_async_with_config("ws://localhost/rpc", stream, Some(config))
                    .await.map_err(|error| format!("upgrading socket connection: {error}"))
            };
            let (socket, response) = tokio::select! {
                result = tokio::time::timeout(STARTING, connect) => match result {
                    Ok(result) => result?,
                    Err(_) => return Err(format!("the app-server did not take the connection within {} s", STARTING.as_secs())),
                },
                _ = stopping.wait_for(|stopped| *stopped) => return Ok(()),
            };
            let frame_limit = response.headers().get("x-codex-websocket-max-unfragmented-message-bytes")
                .map(|value| value.to_str().map_err(|error| error.to_string())?.parse::<usize>().map_err(|error| error.to_string()))
                .transpose()?;
            let (mut writer, mut reader) = socket.split();
            let read = async {
                loop {
                    let message = tokio::select! {
                        // Apply already-readable notifications before reporting
                        // the process's exit, without waiting for socket EOF.
                        biased;
                        message = reader.next() => message,
                        _ = stopping.wait_for(|stopped| *stopped) => break,
                    };
                    let Some(message) = message else { break };
                    match message.map_err(|error| format!("reading socket: {error}"))? {
                        Message::Text(text) => route(text.as_bytes()),
                        Message::Binary(bytes) => route(&bytes),
                        Message::Ping(bytes) => { let _ = replies.send(Message::Pong(bytes)); }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
                Err::<(), String>("the node's app-server socket connection closed".into())
            };
            let write = async {
                let mut messages = messages;
                while let Some(message) = messages.recv().await {
                    if let (Some(limit), Message::Text(text)) = (frame_limit, &message)
                        && text.len() > limit {
                        if limit == 0 {
                            return Err("app-server advertised a zero frame limit".into());
                        }
                        let mut chunks = text.as_bytes().chunks(limit).peekable();
                        let mut opcode = OpCode::Data(Data::Text);
                        while let Some(bytes) = chunks.next() {
                            let frame = Frame::message(bytes.to_vec(), opcode, chunks.peek().is_none());
                            writer.send(Message::Frame(frame)).await.map_err(|error| format!("writing socket: {error}"))?;
                            opcode = OpCode::Data(Data::Continue);
                        }
                    } else {
                        writer.send(message).await.map_err(|error| format!("writing socket: {error}"))?;
                    }
                }
                Err::<(), String>("the node's app-server sender closed".into())
            };
            tokio::pin!(read, write);
            tokio::select! {
                result = &mut read => result,
                result = &mut write => {
                    kill(&held, "app-server", &log);
                    let _ = read.await;
                    result
                }
            }
        }.await;
        if let Err(error) = run {
            log(format!("Codex socket connection ended: {error}"));
        }
        kill(&held, "app-server", &log);
        let why = exited.await.unwrap_or_else(|_| "its exit watcher stopped".into());
        let _ = finished.send(why);
    });
    Ok(Transport { child, outgoing, exited: drained })
}

fn kill(child: &Child, name: &str, log: &Log) {
    if let Err(error) = child.kill() {
        log(format!("stopping Codex {name}: {error}"));
    }
}

fn read_log(pipe: io::PipeReader, name: &'static str, log: Log, ready: Option<oneshot::Sender<()>>) -> io::Result<()> {
    std::thread::Builder::new().name(name.into()).spawn(move || {
        let mut ready = ready;
        for line in BufReader::new(pipe).lines() {
            match line {
                Ok(line) => {
                    if ready.is_some()
                        && serde_json::from_str::<Value>(&line).ok().is_some_and(|value| {
                            value["target"] == LISTENER_TARGET && value["fields"]["message"] == LISTENING
                        })
                    {
                        let _ = ready.take().expect("checked above").send(());
                    }
                    log(format!("{name}: {line}"));
                }
                Err(error) => {
                    log(format!("reading {name}: {error}"));
                    return;
                }
            }
        }
    })?;
    Ok(())
}
