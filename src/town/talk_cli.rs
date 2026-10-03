//! `fastctx talk`: the user talks to a resident from a terminal and watches it answer.

use super::admin::TownRequest;
use super::talk::TalkEvent;
use crate::world::node::admin::{self, AdminRequest};
use std::io::Write as _;
use std::process::ExitCode;

#[derive(Debug, clap::Args)]
pub(crate) struct TalkArgs {
    /// Open a new resident first: codex or claude. Every word given is then its first words.
    #[arg(long, value_name = "AGENT")]
    open: Option<String>,
    /// The town to open it in; omitted, outside every town.
    #[arg(long, requires = "open")]
    town: Option<String>,
    /// The machine to open it on; omitted, this one.
    #[arg(long, requires = "open")]
    node: Option<String>,
    /// The resident to talk to, such as codex-7f3a.
    resident: Option<String>,
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
    let (resident, words, opened) = match args.open {
        Some(agent) => {
            let words: Vec<String> = args.resident.into_iter().chain(args.words).collect();
            if words.is_empty() {
                return Err("Give the first words for the new resident.".to_string());
            }
            let request = AdminRequest::Town { request: TownRequest::Open { agent, town: args.town, words: words.join(" "), node: args.node } };
            let resident = match admin::call(&endpoint, &request, admin::ADMIN_TIMEOUT).await? {
                Some(response) if response.ok => response.data.get("resident").and_then(serde_json::Value::as_str).unwrap_or_default().to_string(),
                Some(response) => return Err(response.error.unwrap_or_else(|| "The node could not open the resident.".to_string())),
                None => return Err("The FastCtx node service is not running on this machine; run 'fastctx node status'.".to_string()),
            };
            println!("=== opened {resident} ===");
            (resident, Vec::new(), true)
        }
        None => (args.resident.ok_or("Name the resident to talk to, or open one with --open.")?, args.words, false),
    };
    let mut watch = crate::runtime::local_ipc::connect(&endpoint)
        .await
        .map_err(|_| "The FastCtx node service is not running on this machine; run 'fastctx node status'.".to_string())?;
    let request = AdminRequest::Town { request: TownRequest::Watch { resident: resident.clone() } };
    admin::write_frame(&mut watch, &serde_json::to_vec(&request).map_err(|error| error.to_string())?).await?;
    // Frames are read by their own task: reading one must never be cut short by a typed line,
    // or the rest of the frame would be read as the start of the next.
    let (events_tx, mut events) = tokio::sync::mpsc::unbounded_channel::<Result<TalkEvent, String>>();
    tokio::spawn(async move {
        loop {
            let event = next(&mut watch).await;
            let failed = event.is_err();
            if events_tx.send(event).is_err() || failed {
                return;
            }
        }
    });
    let mut next_event = async || events.recv().await.unwrap_or_else(|| Err("The node closed the watch.".to_string()));

    let mut printer = Printer::default();
    // The history comes first; words said before it ends would show twice.
    let mut running: Option<String> = None;
    loop {
        let event = next_event().await?;
        match &event {
            TalkEvent::TurnStarted { turn } => running = Some(turn.clone()),
            TalkEvent::TurnEnded { .. } => running = None,
            _ => {}
        }
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
        let resident = resident.clone();
        async move {
            let request = AdminRequest::Town { request: TownRequest::Say { resident, text } };
            match admin::call(&endpoint, &request, admin::ADMIN_TIMEOUT).await? {
                Some(response) if response.ok => Ok(response.data.get("turn").and_then(serde_json::Value::as_str).map(str::to_string)),
                Some(response) => Err(response.error.unwrap_or_else(|| "The node refused the words.".to_string())),
                None => Err("The FastCtx node service stopped.".to_string()),
            }
        }
    };

    // Follows the session until `turn` ends; without a known turn id (a Claude resident), until
    // the first turn that starts from now on ends.
    let mut follow = async |turn: Option<String>, printer: &mut Printer| -> Result<ExitCode, String> {
        let mut target = turn;
        loop {
            let event = next_event().await?;
            if target.is_none()
                && let TalkEvent::TurnStarted { turn } = &event
            {
                target = Some(turn.clone());
            }
            let done = match &event {
                TalkEvent::TurnEnded { turn, .. } => target.as_ref() == Some(turn),
                TalkEvent::Gone { .. } => true,
                _ => false,
            };
            printer.show(&event);
            if done {
                return Ok(ExitCode::SUCCESS);
            }
        }
    };

    if opened {
        // Its first turn started before the watch; follow it unless it already ended.
        return match running {
            Some(turn) => follow(Some(turn), &mut printer).await,
            None => Ok(ExitCode::SUCCESS),
        };
    }
    if !words.is_empty() {
        let turn = say(words.join(" ")).await?;
        return follow(turn, &mut printer).await;
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
            event = next_event() => {
                let event = event?;
                let gone = matches!(event, TalkEvent::Gone { .. });
                printer.show(&event);
                if gone {
                    return Ok(ExitCode::FAILURE);
                }
            }
            line = lines.recv() => match line {
                Some(line) if line.trim().is_empty() => {}
                Some(line) => {
                    say(line).await?;
                }
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
    /// The last turn shown as started; a turn running when the watch began appears in the
    /// history and again live.
    turn: Option<String>,
}

impl Printer {
    fn show(&mut self, event: &TalkEvent) {
        if let TalkEvent::TurnStarted { turn } = event {
            if self.turn.as_ref() == Some(turn) {
                return;
            }
            self.turn = Some(turn.clone());
        }
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
                    TalkEvent::Words { from, text, .. } => format!("> {from}: {text}"),
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
