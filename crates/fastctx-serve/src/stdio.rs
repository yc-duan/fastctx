//! MCP's stdio transport: one JSON-RPC message per line in each direction.
//!
//! Standard input is read on a thread of its own, outside the async
//! runtime: a writer the host leaked to another process can keep a read
//! waiting forever, and that must not hold up anything else. A read error
//! is kept apart from the end of input, so neither is mistaken for the
//! other. Standard output has one writer, so lines never interleave.
//!
//! The host's process ending is a second sign the session is over: a host
//! that dies without its writer closing, because another process inherited
//! it, would otherwise leave this server running for good.

use std::io::{self, BufRead as _};

use tokio::io::AsyncWriteExt as _;
use tokio::sync::{mpsc, oneshot};

/// What reading standard input yields: a line, or why reading stopped.
pub enum Input {
    Line(String),
    Failed(io::Error),
    /// The process that started this server exited.
    HostGone,
}

/// Lines from standard input until it ends.
pub fn input() -> mpsc::Receiver<Input> {
    let (sender, receiver) = mpsc::channel(16);
    // Where the platform cannot watch a process, standard input is the only
    // sign, as it always was.
    if let Ok(host) = fastctx_platform::host_process::parent_identity() {
        let sender = sender.clone();
        std::thread::spawn(move || {
            if fastctx_platform::host_process::wait_for_exit(&host).is_ok() {
                let _ = sender.blocking_send(Input::HostGone);
            }
        });
    }
    std::thread::spawn(move || {
        let stdin = io::stdin();
        let mut stdin = stdin.lock();
        loop {
            let mut line = String::new();
            match stdin.read_line(&mut line) {
                Ok(0) => return,
                Ok(_) => {
                    if sender.blocking_send(Input::Line(line)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = sender.blocking_send(Input::Failed(error));
                    return;
                }
            }
        }
    });
    receiver
}

/// The one writer of standard output.
#[derive(Clone)]
pub struct Output {
    sender: mpsc::UnboundedSender<Line>,
}

struct Line {
    text: String,
    receipt: Option<oneshot::Sender<Result<(), String>>>,
    answer: Option<crate::answer::Answer>,
}

impl Output {
    /// Starts the writer; the returned task ends once every `Output` is
    /// dropped and every line is written.
    pub fn start() -> (Self, tokio::task::JoinHandle<io::Result<()>>) {
        let (sender, mut receiver) = mpsc::unbounded_channel::<Line>();
        let writer = tokio::spawn(async move {
            let mut stdout = tokio::io::stdout();
            while let Some(Line { text: mut line, receipt, answer }) = receiver.recv().await {
                line.push('\n');
                let written = async {
                    stdout.write_all(line.as_bytes()).await?;
                    stdout.flush().await
                }
                .await;
                if written.is_ok()
                    && let Some(answer) = answer
                {
                    answer.sent().await;
                }
                if let Some(receipt) = receipt {
                    let _ = receipt.send(match &written {
                        Ok(()) => Ok(()),
                        Err(error) => Err(error.to_string()),
                    });
                }
                written?;
            }
            Ok(())
        });
        (Self { sender }, writer)
    }

    // The queue owns the collection until the write finishes. Cancelling
    // the tool task cannot release words while their response still writes.
    pub async fn send_answer(&self, message: &serde_json::Value, answer: crate::answer::Answer) -> Result<(), String> {
        let (written, receipt) = oneshot::channel();
        self.sender
            .send(Line { text: message.to_string(), receipt: Some(written), answer: Some(answer) })
            .map_err(|error| error.to_string())?;
        receipt.await.map_err(|error| error.to_string())?
    }

    pub fn send(&self, message: &serde_json::Value) {
        // The writer stops only when standard output fails, and then there
        // is no one left to tell.
        let _ = self.sender.send(Line { text: message.to_string(), receipt: None, answer: None });
    }
}
