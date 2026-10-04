//! MCP's stdio transport: one JSON-RPC message per line in each direction.
//!
//! Standard input is read on a thread of its own, outside the async
//! runtime: a writer the host leaked to another process can keep a read
//! waiting forever, and that must not hold up anything else. A read error
//! is kept apart from the end of input, so neither is mistaken for the
//! other. Standard output has one writer, so lines never interleave.

use std::io::{self, BufRead as _};

use tokio::io::AsyncWriteExt as _;
use tokio::sync::mpsc;

/// What reading standard input yields: a line, or why reading stopped.
pub enum Input {
    Line(String),
    Failed(io::Error),
}

/// Lines from standard input until it ends.
pub fn input() -> mpsc::Receiver<Input> {
    let (sender, receiver) = mpsc::channel(16);
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
    sender: mpsc::UnboundedSender<String>,
}

impl Output {
    /// Starts the writer; the returned task ends once every `Output` is
    /// dropped and every line is written.
    pub fn start() -> (Self, tokio::task::JoinHandle<io::Result<()>>) {
        let (sender, mut receiver) = mpsc::unbounded_channel::<String>();
        let writer = tokio::spawn(async move {
            let mut stdout = tokio::io::stdout();
            while let Some(mut line) = receiver.recv().await {
                line.push('\n');
                stdout.write_all(line.as_bytes()).await?;
                stdout.flush().await?;
            }
            Ok(())
        });
        (Self { sender }, writer)
    }

    pub fn send(&self, message: &serde_json::Value) {
        // The writer stops only when standard output fails, and then there
        // is no one left to tell.
        let _ = self.sender.send(message.to_string());
    }
}
