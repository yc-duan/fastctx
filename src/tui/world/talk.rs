//! The live conversation behind the chat screen: one resident on this machine, watched from
//! this terminal (`specs/world-colleagues.md`, "用户在任何地方和居民说话").
//!
//! A background thread with its own runtime opens the resident when it is new, holds the node's
//! watch on it, and sends the user's lines as the user's own words; the UI thread drains what
//! arrived on each tick into a transcript it can draw at any width. Dropping the client ends
//! the watch.

use crate::town::admin::TownRequest;
use crate::town::talk::TalkEvent;
use crate::world::node::admin::{self, AdminRequest};
use std::sync::mpsc;

/// Who the conversation is with.
pub(super) enum Target {
    Resident(String),
    /// A resident to open first, on this machine or `node`; `words` are the first it hears.
    New { agent: String, town: Option<String>, node: Option<String>, words: String },
}

/// Why a conversation ended.
pub(super) enum Closed {
    /// The node service is not running, or stopped.
    NodeDown,
    /// The node ended the watch, or the resident's events ended.
    Ended(Option<String>),
    /// Something failed on the way, in the words of whatever failed.
    Failed(String),
}

enum Command {
    Say(String),
    Stop,
}

enum Update {
    Opened(String),
    Event(TalkEvent),
    /// A line or a stop was refused; the reason, for the transcript.
    Refused(String),
    Closed(Closed),
}

pub(super) struct TalkClient {
    /// The resident, once known: at once for an existing one, once opened for a new one.
    pub(super) resident: Option<String>,
    updates: mpsc::Receiver<Update>,
    commands: tokio::sync::mpsc::UnboundedSender<Command>,
    pub(super) transcript: Transcript,
}

impl TalkClient {
    pub(super) fn open(target: Target) -> Self {
        let resident = match &target {
            Target::Resident(resident) => Some(resident.clone()),
            Target::New { .. } => None,
        };
        let (updates_tx, updates) = mpsc::channel();
        let (commands, commands_rx) = tokio::sync::mpsc::unbounded_channel();
        let failed = updates_tx.clone();
        let spawned = std::thread::Builder::new()
            .name("fastctx-world-talk".into())
            .spawn(move || serve(target, &updates_tx, commands_rx));
        if let Err(error) = spawned {
            let _ = failed.send(Update::Closed(Closed::Failed(error.to_string())));
        }
        Self { resident, updates, commands, transcript: Transcript::default() }
    }

    /// A client with a given transcript and no conversation behind it, for drawing.
    #[cfg(test)]
    pub(super) fn detached(resident: &str, transcript: Transcript) -> Self {
        let (_, updates) = mpsc::channel();
        let (commands, _) = tokio::sync::mpsc::unbounded_channel();
        Self { resident: Some(resident.to_string()), updates, commands, transcript }
    }

    /// Says one line the user typed, as the user.
    pub(super) fn say(&mut self, line: String) {
        if self.commands.send(Command::Say(line)).is_err() && self.transcript.closed.is_none() {
            self.transcript.closed = Some(Closed::Ended(None));
        }
    }

    /// Stops the turn the resident is running.
    pub(super) fn stop(&mut self) {
        let _ = self.commands.send(Command::Stop);
    }

    /// Takes in everything that arrived; true when anything changed.
    pub(super) fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(update) = self.updates.try_recv() {
            changed = true;
            match update {
                Update::Opened(resident) => self.resident = Some(resident),
                Update::Event(event) => self.transcript.push(event),
                Update::Refused(reason) => self.transcript.entries.push(Entry::Refused(reason)),
                Update::Closed(closed) => self.transcript.closed = Some(closed),
            }
        }
        changed
    }
}

fn serve(target: Target, updates: &mpsc::Sender<Update>, mut commands: tokio::sync::mpsc::UnboundedReceiver<Command>) {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = updates.send(Update::Closed(Closed::Failed(error.to_string())));
            return;
        }
    };
    runtime.block_on(async move {
        let closed = converse(target, updates, &mut commands).await;
        let _ = updates.send(Update::Closed(closed));
    });
}

/// Holds one conversation until it ends, and says why it ended.
async fn converse(target: Target, updates: &mpsc::Sender<Update>, commands: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> Closed {
    let endpoint = match crate::session::SessionEnvironment::capture().and_then(|environment| crate::runtime::node_admin_endpoint(&environment)) {
        Ok(endpoint) => endpoint,
        Err(error) => return Closed::Failed(error),
    };
    let ask = async |request: TownRequest| -> Result<serde_json::Value, Closed> {
        match admin::call(&endpoint, &AdminRequest::Town { request }, admin::ADMIN_TIMEOUT).await {
            Ok(Some(response)) if response.ok => Ok(response.data),
            Ok(Some(response)) => Err(Closed::Failed(response.error.unwrap_or_default())),
            Ok(None) => Err(Closed::NodeDown),
            Err(error) => Err(Closed::Failed(error)),
        }
    };
    let resident = match target {
        Target::Resident(resident) => resident,
        Target::New { agent, town, node, words } => {
            let opened = match ask(TownRequest::Open { agent, town, words, node }).await {
                Ok(data) => data.get("resident").and_then(serde_json::Value::as_str).unwrap_or_default().to_string(),
                Err(closed) => return closed,
            };
            if updates.send(Update::Opened(opened.clone())).is_err() {
                return Closed::Ended(None);
            }
            opened
        }
    };
    let Ok(mut watch) = crate::runtime::local_ipc::connect(&endpoint).await else { return Closed::NodeDown };
    let request = AdminRequest::Town { request: TownRequest::Watch { resident: resident.clone() } };
    let bytes = serde_json::to_vec(&request).expect("an admin request encodes");
    if let Err(error) = admin::write_frame(&mut watch, &bytes).await {
        return Closed::Failed(error);
    }
    // Frames are read by their own task: a read cut short by a typed line would leave the rest
    // of the frame to be read as the start of the next.
    let (events_tx, mut events) = tokio::sync::mpsc::unbounded_channel::<Result<TalkEvent, Closed>>();
    tokio::spawn(async move {
        loop {
            let event = match admin::read_frame(&mut watch).await {
                Ok(bytes) => serde_json::from_slice::<TalkEvent>(&bytes).map_err(|error| Closed::Failed(error.to_string())),
                Err(_) => Err(Closed::Ended(None)),
            };
            let failed = event.is_err();
            if events_tx.send(event).is_err() || failed {
                return;
            }
        }
    });
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Some(Ok(event)) => {
                    if updates.send(Update::Event(event)).is_err() {
                        return Closed::Ended(None);
                    }
                }
                Some(Err(closed)) => return closed,
                None => return Closed::Ended(None),
            },
            command = commands.recv() => {
                let request = match command {
                    Some(Command::Say(text)) => TownRequest::Say { resident: resident.clone(), text },
                    Some(Command::Stop) => TownRequest::Interrupt { resident: resident.clone() },
                    None => return Closed::Ended(None),
                };
                if let Err(refused) = ask(request).await {
                    let reason = match refused {
                        Closed::Failed(reason) => reason,
                        Closed::NodeDown => return Closed::NodeDown,
                        Closed::Ended(_) => return Closed::Ended(None),
                    };
                    if updates.send(Update::Refused(reason)).is_err() {
                        return Closed::Ended(None);
                    }
                }
            }
        }
    }
}

/// The conversation as the chat screen draws it.
#[derive(Default)]
pub(super) struct Transcript {
    pub(super) entries: Vec<Entry>,
    /// Whether the history has been given and what follows is live.
    pub(super) live: bool,
    /// Why the conversation ended, when it has.
    pub(super) closed: Option<Closed>,
    /// Whether a turn is running now.
    pub(super) busy: bool,
    /// The resident's state as the node last reported it.
    pub(super) state: Option<String>,
}

pub(super) enum Entry {
    Words { from: String, text: String },
    /// A message of the model; `streaming` while its text is still arriving.
    Message { item: String, text: String, streaming: bool },
    Tool { item: String, name: String, summary: String, state: ToolState },
    /// A turn ended, and why when it did not complete.
    TurnEnded { status: String, reason: Option<String> },
    /// What the host or the World reported about the session itself.
    Health(String),
    Refused(String),
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum ToolState {
    Running,
    Done,
    Failed,
}

impl Transcript {
    pub(super) fn push(&mut self, event: TalkEvent) {
        match event {
            TalkEvent::Words { from, text, .. } => self.entries.push(Entry::Words { from, text }),
            TalkEvent::TurnStarted { .. } => self.busy = true,
            TalkEvent::Delta { item, text } => match self.streaming(&item) {
                Some(so_far) => so_far.push_str(&text),
                None => self.entries.push(Entry::Message { item, text, streaming: true }),
            },
            TalkEvent::Message { item, text } => {
                let found = self.entries.iter_mut().rev().find_map(|entry| match entry {
                    Entry::Message { item: current, text: so_far, streaming } if *current == item => Some((so_far, streaming)),
                    _ => None,
                });
                match found {
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
            TalkEvent::TurnEnded { status, reason, .. } => {
                self.busy = false;
                for entry in &mut self.entries {
                    if let Entry::Message { streaming, .. } = entry {
                        *streaming = false;
                    }
                }
                self.entries.push(Entry::TurnEnded { status, reason });
            }
            TalkEvent::Health { text } => self.entries.push(Entry::Health(text)),
            TalkEvent::State { state } => {
                self.busy = state == "busy";
                self.state = Some(state);
            }
            TalkEvent::Live => self.live = true,
            TalkEvent::Gone { reason } => self.closed = Some(Closed::Ended(Some(reason))),
        }
    }

    /// The text of a message still arriving for `item`, if there is one.
    fn streaming(&mut self, item: &str) -> Option<&mut String> {
        self.entries.iter_mut().rev().find_map(|entry| match entry {
            Entry::Message { item: current, text, streaming: true } if current == item => Some(text),
            _ => None,
        })
    }
}
