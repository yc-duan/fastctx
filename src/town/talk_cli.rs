//! `fastctx talk`: the user talks to a resident from a terminal and watches it answer.

use super::admin::TownRequest;
use super::talk::TalkEvent;
use crate::world::node::admin::{self, AdminRequest};
use std::io::Write as _;
use std::process::ExitCode;

#[derive(Debug, clap::Args)]
pub(crate) struct TalkArgs {
    /// The resident to talk to, such as codex-7f3a.
    resident: String,
    /// Words to say. Given, the command says them, shows the turn they start or join, and
    /// exits when it ends; omitted, every line typed is said, until end of input.
    words: Vec<String>,
}

pub(crate) async fn run(args: TalkArgs) -> Result<ExitCode, String> {
    // Words go in as the user's own; a resident's command must not speak for the user.
    if std::env::var_os(super::session::RESIDENT_VARIABLE).is_some_and(|value| !value.is_empty()) {
        return Err("fastctx talk is the user's own voice; residents speak to each other with message_colleague.".to_string());
    }
    let environment = crate::session::SessionEnvironment::capture()?;
    let endpoint = crate::runtime::node_admin_endpoint(&environment)?;
    let mut watch = crate::runtime::local_ipc::connect(&endpoint)
        .await
        .map_err(|_| "The FastCtx node service is not running on this machine; run 'fastctx node status'.".to_string())?;
    let request = AdminRequest::Town { request: TownRequest::Watch { resident: args.resident.clone() } };
    admin::write_frame(&mut watch, &serde_json::to_vec(&request).map_err(|error| error.to_string())?).await?;

    let mut printer = Printer::default();
    // The history comes first; words said before it ends would show twice.
    loop {
        let event = next(&mut watch).await?;
        let live = matches!(event, TalkEvent::Live);
        let gone = matches!(event, TalkEvent::Gone { .. });
        printer.show(&event);
        if gone {
            return Ok(ExitCode::FAILURE);
        }
        if live {
            break;
        }
    }
    let say = |text: String| {
        let endpoint = endpoint.clone();
        let resident = args.resident.clone();
        async move {
            let request = AdminRequest::Town { request: TownRequest::Say { resident, text } };
            match admin::call(&endpoint, &request, admin::ADMIN_TIMEOUT).await? {
                Some(response) if response.ok => Ok(()),
                Some(response) => Err(response.error.unwrap_or_else(|| "The node refused the words.".to_string())),
                None => Err("The FastCtx node service stopped.".to_string()),
            }
        }
    };

    if !args.words.is_empty() {
        say(args.words.join(" ")).await?;
        // Follow the session until the turn the words went into ends.
        let mut said = false;
        loop {
            let event = next(&mut watch).await?;
            if let TalkEvent::Words { from, .. } = &event
                && from == "user"
            {
                said = true;
            }
            let ended = matches!(event, TalkEvent::TurnEnded { .. } | TalkEvent::Gone { .. });
            printer.show(&event);
            if said && ended {
                return Ok(ExitCode::SUCCESS);
            }
        }
    }

    let (lines_tx, mut lines) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines().map_while(Result::ok) {
            if lines_tx.send(line).is_err() {
                return;
            }
        }
    });
    loop {
        tokio::select! {
            event = next(&mut watch) => {
                let event = event?;
                let gone = matches!(event, TalkEvent::Gone { .. });
                printer.show(&event);
                if gone {
                    return Ok(ExitCode::FAILURE);
                }
            }
            line = lines.recv() => match line {
                Some(line) if line.trim().is_empty() => {}
                Some(line) => say(line).await?,
                None => return Ok(ExitCode::SUCCESS),
            },
        }
    }
}

async fn next(watch: &mut crate::runtime::local_ipc::BoxedStream) -> Result<TalkEvent, String> {
    let bytes = admin::read_frame(watch).await.map_err(|_| "The node closed the watch.".to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| format!("Unreadable event from the node: {error}"))
}

/// Prints events as a plain transcript: streamed text inline, tools and turn edges on their
/// own lines.
#[derive(Default)]
struct Printer {
    /// The message item whose text is being streamed, if any.
    streaming: Option<String>,
}

impl Printer {
    fn show(&mut self, event: &TalkEvent) {
        let mut out = std::io::stdout().lock();
        match event {
            TalkEvent::Delta { item, text } => {
                if self.streaming.as_deref() != Some(item) {
                    self.end_line(&mut out);
                    self.streaming = Some(item.clone());
                }
                let _ = write!(out, "{text}");
            }
            TalkEvent::Message { item, text } => {
                if self.streaming.as_deref() == Some(item) {
                    let _ = writeln!(out);
                    self.streaming = None;
                } else {
                    self.end_line(&mut out);
                    let _ = writeln!(out, "{text}");
                }
            }
            other => {
                self.end_line(&mut out);
                let line = match other {
                    TalkEvent::Words { from, text } => format!("> {from}: {text}"),
                    TalkEvent::TurnStarted { .. } => "── turn started".to_string(),
                    TalkEvent::TurnEnded { status, .. } => format!("── turn {status}"),
                    TalkEvent::Tool { name, summary, done: false, .. } => format!("  {name} {summary}"),
                    TalkEvent::Tool { name, failed: true, .. } => format!("  {name} failed"),
                    TalkEvent::Tool { .. } => return,
                    TalkEvent::State { state } => format!("── {state}"),
                    TalkEvent::Gone { reason } => format!("── {reason}"),
                    TalkEvent::Live => "── now".to_string(),
                    TalkEvent::Delta { .. } | TalkEvent::Message { .. } => return,
                };
                let _ = writeln!(out, "{line}");
            }
        }
        let _ = out.flush();
    }

    fn end_line(&mut self, out: &mut impl std::io::Write) {
        if self.streaming.take().is_some() {
            let _ = writeln!(out);
        }
    }
}
