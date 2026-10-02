//! The live conversation behind the chat panel: one resident watched from this terminal.
//!
//! A background thread with its own runtime holds the node's watch on the resident and sends
//! the user's lines as the user's own words; the UI thread drains what arrived on each tick
//! into a transcript it can draw at any width. Closing the panel drops the client, which ends
//! the watch.

use crate::town::admin::TownRequest;
use crate::town::talk::TalkEvent;
use crate::world::node::admin::{self, AdminRequest};
use std::sync::mpsc;

/// What the background thread hands the UI.
enum Update {
    Event(TalkEvent),
    /// The user's line was refused; the reason, for the panel.
    Refused(String),
    /// The watch ended.
    Closed(String),
}

pub(crate) struct TalkClient {
    pub(crate) resident: String,
    updates: mpsc::Receiver<Update>,
    lines: tokio::sync::mpsc::UnboundedSender<String>,
    pub(crate) transcript: Transcript,
}

impl TalkClient {
    /// Starts watching `resident` on this machine.
    pub(crate) fn open(resident: &str) -> Self {
        let (updates_tx, updates) = mpsc::channel();
        let (lines, lines_rx) = tokio::sync::mpsc::unbounded_channel();
        let watched = resident.to_string();
        let spawned = std::thread::Builder::new()
            .name("fastctx-world-talk".into())
            .spawn(move || serve(&watched, &updates_tx, lines_rx));
        let mut transcript = Transcript::default();
        if let Err(error) = spawned {
            transcript.closed = Some(format!("Cannot start the conversation: {error}"));
        }
        Self { resident: resident.to_string(), updates, lines, transcript }
    }

    /// Sends one line the user typed.
    pub(crate) fn say(&mut self, line: String) {
        if self.lines.send(line).is_err() {
            self.transcript.closed.get_or_insert_with(|| "The conversation has ended.".to_string());
        }
    }

    /// Takes in everything that arrived; true when the transcript changed.
    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(update) = self.updates.try_recv() {
            changed = true;
            match update {
                Update::Event(event) => self.transcript.push(event),
                Update::Refused(reason) => self.transcript.entries.push(Entry::Notice(reason)),
                Update::Closed(reason) => self.transcript.closed = Some(reason),
            }
        }
        changed
    }
}

fn serve(resident: &str, updates: &mpsc::Sender<Update>, mut lines: tokio::sync::mpsc::UnboundedReceiver<String>) {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = updates.send(Update::Closed(format!("Cannot start the conversation: {error}")));
            return;
        }
    };
    runtime.block_on(async move {
        let endpoint = match crate::session::SessionEnvironment::capture().and_then(|environment| crate::runtime::node_admin_endpoint(&environment)) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                let _ = updates.send(Update::Closed(error));
                return;
            }
        };
        let mut watch = match crate::runtime::local_ipc::connect(&endpoint).await {
            Ok(watch) => watch,
            Err(_) => {
                let _ = updates.send(Update::Closed("The FastCtx node service is not running on this machine.".into()));
                return;
            }
        };
        let request = AdminRequest::Town { request: TownRequest::Watch { resident: resident.to_string() } };
        let bytes = serde_json::to_vec(&request).expect("an admin request encodes");
        if let Err(error) = admin::write_frame(&mut watch, &bytes).await {
            let _ = updates.send(Update::Closed(error));
            return;
        }
        loop {
            tokio::select! {
                frame = admin::read_frame(&mut watch) => {
                    let event = match frame.map_err(|_| "The node closed the conversation.".to_string()).and_then(|bytes| {
                        serde_json::from_slice::<TalkEvent>(&bytes).map_err(|error| format!("Unreadable event from the node: {error}"))
                    }) {
                        Ok(event) => event,
                        Err(reason) => {
                            let _ = updates.send(Update::Closed(reason));
                            return;
                        }
                    };
                    if updates.send(Update::Event(event)).is_err() {
                        return;
                    }
                }
                line = lines.recv() => {
                    let Some(text) = line else { return };
                    let request = AdminRequest::Town { request: TownRequest::Say { resident: resident.to_string(), text } };
                    let refused = match admin::call(&endpoint, &request, admin::ADMIN_TIMEOUT).await {
                        Ok(Some(response)) if response.ok => None,
                        Ok(Some(response)) => Some(response.error.unwrap_or_else(|| "The node refused the words.".to_string())),
                        Ok(None) => Some("The FastCtx node service stopped.".to_string()),
                        Err(error) => Some(error),
                    };
                    if let Some(reason) = refused
                        && updates.send(Update::Refused(reason)).is_err()
                    {
                        return;
                    }
                }
            }
        }
    });
}

/// The conversation as the panel draws it.
#[derive(Default)]
pub(crate) struct Transcript {
    pub(crate) entries: Vec<Entry>,
    /// Whether everything after the history has started arriving.
    pub(crate) live: bool,
    /// Why the conversation ended, when it has.
    pub(crate) closed: Option<String>,
    /// Whether a turn is running now.
    pub(crate) busy: bool,
}

pub(crate) enum Entry {
    Words { from: String, text: String },
    /// A message of the model; `streaming` while its text is still arriving.
    Message { item: String, text: String, streaming: bool },
    Tool { item: String, name: String, summary: String, state: ToolState },
    TurnEnded { status: String },
    Notice(String),
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum ToolState {
    Running,
    Done,
    Failed,
}

impl Transcript {
    fn push(&mut self, event: TalkEvent) {
        match event {
            TalkEvent::Words { from, text } => self.entries.push(Entry::Words { from, text }),
            TalkEvent::TurnStarted { .. } => self.busy = true,
            TalkEvent::Delta { item, text } => match self.entries.last_mut() {
                Some(Entry::Message { item: current, text: so_far, streaming: true }) if *current == item => so_far.push_str(&text),
                _ => self.entries.push(Entry::Message { item, text, streaming: true }),
            },
            TalkEvent::Message { item, text } => {
                let streamed = self.entries.iter_mut().rev().find_map(|entry| match entry {
                    Entry::Message { item: current, text: so_far, streaming } if *current == item => Some((so_far, streaming)),
                    _ => None,
                });
                match streamed {
                    Some((so_far, streaming)) => {
                        *so_far = text;
                        *streaming = false;
                    }
                    None => self.entries.push(Entry::Message { item, text, streaming: false }),
                }
            }
            TalkEvent::Tool { item, name, summary, done, failed } => {
                let state = match (done, failed) {
                    (false, _) => ToolState::Running,
                    (true, false) => ToolState::Done,
                    (true, true) => ToolState::Failed,
                };
                let started = self.entries.iter_mut().rev().find_map(|entry| match entry {
                    Entry::Tool { item: current, state, .. } if *current == item => Some(state),
                    _ => None,
                });
                match started {
                    Some(current) => *current = state,
                    None => self.entries.push(Entry::Tool { item, name, summary, state }),
                }
            }
            TalkEvent::TurnEnded { status, .. } => {
                self.busy = false;
                self.entries.push(Entry::TurnEnded { status });
            }
            TalkEvent::State { state } => self.entries.push(Entry::Notice(state)),
            TalkEvent::Live => self.live = true,
            TalkEvent::Gone { reason } => self.closed = Some(reason),
        }
    }
}
