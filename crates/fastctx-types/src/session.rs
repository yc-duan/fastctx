//! What happens in an agent's session, in one form for every host
//! (`world-colleagues.md`, 一轮怎样结束 and 用户在任何地方和居民说话;
//! `world-gui.md`, 它是什么).
//!
//! Each host reports its sessions in its own words: Codex's app-server in
//! notifications about threads, turns and items, Claude Code through the
//! World's mod. Where the World first hears them it translates them into
//! these events, and everything that follows a session (`fastctx talk`, the
//! TUI, a colleague's requester, later the graphical interface) reads only
//! these. Nothing here is made up where the host said nothing; a kind of
//! thing the World does not know is passed on by the host's name for it.
//!
//! A turn is one round of work: it starts when words go into an idle
//! session and ends when the agent stops. Words that arrive while a turn
//! runs join it at its next tool boundary. Within a turn the agent does
//! items, one after another or several at once: it writes, thinks, uses a
//! tool. An item begins, may grow while it goes on, and finishes whole; only
//! those watching live see it grow, and the finished item is all a record
//! keeps.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{Address, MachineName, NoticeId};

pub mod attachments;
pub mod control;
mod details;
pub mod held;
pub use held::Fingerprint;
pub mod images;
pub use details::*;
mod host;
pub use host::*;
mod turn_facts;
pub use turn_facts::*;

/// One thing that happened in a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEvent {
    pub at: Timestamp,
    #[serde(flatten)]
    pub happened: Happened,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum Happened {
    TurnStarted {
        turn: String,
    },
    /// Words went into the session. `from` is unknown where nothing tells,
    /// as for words a host's record holds that the World did not send.
    Heard {
        #[serde(default)]
        delivery: Option<NoticeId>,
        turn: Option<String>,
        from: Option<Speaker>,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<Box<UserMessage>>,
        /// What the World told, as facts, when these words are its telling:
        /// an interface shows these instead of reading `text`, which is
        /// written for the model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notice: Option<Box<crate::residents::Notice>>,
    },
    /// The agent began an item, as far as it was known then.
    Began {
        turn: String,
        item: Item,
    },
    /// More of an item that is still going: text appended to what it has.
    Grew {
        /// Distinguishes equal chunks emitted within the same timestamp.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revision: Option<u64>,
        turn: String,
        item: String,
        part: Part,
        text: String,
    },
    /// An item, whole, as it ended.
    Finished {
        turn: String,
        item: Item,
    },
    /// Replaces a running item's snapshot, without completing it.
    Updated {
        turn: String,
        item: Item,
    },
    /// Presentation state rather than an additional chat message. Nested
    /// third-party payloads retain their published JSON contracts.
    Changed {
        turn: Option<String>,
        change: SessionChange,
    },
    /// Voice history has its own session and item identities, without a turn.
    Realtime {
        item: serde_json::Value,
        complete: bool,
    },
    RealtimeGrew {
        item: String,
        text: String,
    },
    /// The steps the agent set out for its work, as they stand now: Codex's
    /// plan, Claude Code's to-do list.
    Steps {
        turn: String,
        steps: Vec<Step>,
        note: Option<String>,
    },
    /// How much of its context the session holds, as its host counts it.
    Context {
        used: u64,
        window: Option<u64>,
    },
    /// The title the host gave the session.
    Titled {
        title: String,
    },
    TurnEnded {
        turn: String,
        end: TurnEnd,
    },
    /// Something the host reported about the session rather than its work.
    Health {
        turn: Option<String>,
        health: Health,
    },
    /// Words were handed to the host, and whether a turn took them cannot
    /// be known.
    DeliveryUncertain {
        delivery: NoticeId,
        why: String,
    },
}

impl Happened {
    /// Whether it is kept in a session's record: what grows an item is
    /// only for those watching live, as the finished item holds it all.
    pub fn kept(&self) -> bool {
        !matches!(
            self,
            Happened::Began { .. }
                | Happened::Changed {
                    change: SessionChange::RunFactsUpdated { .. }
                        | SessionChange::Composer { .. }
                        | SessionChange::ComposerFailed { .. }
                        | SessionChange::CommandsChanged
                        | SessionChange::AgentEvent { .. }
                        | SessionChange::AgentStarted { .. }
                        | SessionChange::AgentEnded { .. }
                        | SessionChange::Agents { .. },
                    ..
                }
                | Happened::Grew { .. }
                | Happened::Updated { .. }
                | Happened::RealtimeGrew { .. }
                | Happened::Realtime { complete: false, .. }
        )
    }

    /// Whether a history read from the host's record keeps it: what a
    /// session's record keeps, and what the host's record tells of the
    /// agents the conversation started, which the World does not keep as
    /// the host's record holds it: their starts as read there, not as they
    /// were seen, and their ends.
    pub fn in_history(&self) -> bool {
        self.kept()
            || matches!(
                self,
                Happened::Changed { change: SessionChange::AgentStarted { agent }, .. } if !agent.live
            )
            || matches!(self, Happened::Changed { change: SessionChange::AgentEnded { .. }, .. })
    }
}

/// What watching a session hands over: first what happened before, as far
/// as it is kept, then each event as it happens.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Watched {
    /// Negotiated document packets are never part of conversation history.
    Documents {
        frame: Box<crate::composer::sync::Frame>,
    },
    History {
        events: Vec<SessionEvent>,
    },
    Live {
        event: SessionEvent,
    },
    /// For a viewer that said what it holds (`held::Held`): the history is
    /// still the version it holds.
    Unchanged {
        version: String,
    },
    /// For a viewer that said what it holds: the history is the segments
    /// `layout` names, in that order, and these are the ones it did not
    /// hold. Not `complete`, the latest of them came first and the rest
    /// follow in the next piece.
    Segments {
        version: String,
        layout: Vec<held::SegmentId>,
        segments: Vec<held::Segment>,
        complete: bool,
    },
    /// For a viewer that asked for it (the `now` of its watch): whether the
    /// session's machine says it works now, in which turn, and which words
    /// it still keeps on their way in. Told once the history is told and
    /// whenever it changes; what the viewer shows under way gives way to it,
    /// so one end it missed holds nothing up. A viewer that did not ask, as
    /// one from before this, is never sent it.
    Now {
        busy: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        kept: Vec<KeptWords>,
    },
}

/// Words a conversation's machine keeps that no turn has taken yet, and how
/// far they came (`Watched::Now`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptWords {
    pub id: crate::NoticeId,
    pub state: WordsState,
    /// Present only after a minute of unsuccessful host startup. The input
    /// is still being sent; this is not a failed delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_retry: Option<HostRetry>,
    /// How many turns of its agent login go before the one these words
    /// would start, while that waits for room in the login
    /// (`world-usage.md`, 一个登录同时开几轮).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued: Option<u32>,
    /// Whether that turn waits for room in its machine's memory rather than
    /// in its login (`world-lifecycle.md`, 内存).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub for_memory: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRetry {
    pub since: Timestamp,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WordsState {
    /// They wait for a way in: the host is busy, asleep or not reached.
    Waiting,
    /// They are being handed to the host now, or the host has them and has
    /// not said which turn takes them.
    Handing,
    /// A state of a newer version.
    #[serde(other)]
    Unknown,
}

/// Who said words that went into a session.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Speaker {
    Page {
        page: Box<crate::pages::PageSource>,
    },
    /// The user, typing on `machine`.
    User {
        machine: MachineName,
    },
    /// Another resident.
    Resident {
        address: Address,
    },
    /// The World, telling a fact, such as that a job ended.
    World,
}

/// Something the agent does in a turn. Identity is the pair (turn, id).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    /// The name it was shown under while it streamed, before its host kept
    /// it under this one: whoever holds that item holds this one instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    /// Where the item first appeared, so completed-only history preserves
    /// the presentation order of overlapping items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Timestamp>,
    /// Additional host data, without interpretation or loss: for example
    /// a compaction's summary and counts, or a tool's structured result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
    #[serde(flatten)]
    pub what: What,
}

impl Item {
    /// Thinking that ended at `end` took as long as it ran from its start,
    /// where the host said when it started and not how long it took.
    pub fn thought_until(&mut self, end: Timestamp) {
        if let (What::Thinking { duration_ms: duration @ None, .. }, Some(start)) = (&mut self.what, self.started_at) {
            *duration = u64::try_from(end.duration_since(start).as_millis()).ok();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum What {
    /// Words the agent wrote to whoever it works for.
    Message {
        text: String,
    },
    /// What the host shows of the agent's thinking: Codex's summaries of its
    /// reasoning, Claude's thinking; empty where the host kept the words to
    /// itself.
    Thinking {
        text: String,
        /// How long the model thought, as the host measured it or as the
        /// item's start and end say; none where neither is known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
    /// A plan the agent proposed, whole.
    Plan {
        text: String,
    },
    Tool(Box<Tool>),
    /// The host compacted the session's context.
    Compaction,
    AgentActivity {
        agent: String,
        path: String,
        activity: String,
    },
    /// An item the host deliberately excludes from the conversation. Its
    /// original record stays in `Item.detail` for conditional host panels.
    /// Replacing a visible item with this removes its transient placeholder.
    Hidden {
        host_kind: String,
    },
    /// A host item the World has no common name for.
    Other {
        host_kind: String,
    },
}

/// Which part of an item grows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    /// The text of a message, thinking or plan.
    Text,
    /// What a tool has put out so far.
    Output,
}

/// A tool the agent used.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<Box<crate::run_facts::RunFacts>>,
    /// The tool as its host names it, without the server it belongs to:
    /// `run`, `apply_patch`, `Bash`, `js`.
    pub name: String,
    /// The MCP server or namespace it belongs to; none for the host's own.
    pub server: Option<String>,
    /// The machine it acted on, where the World can tell: its own tools
    /// say, and a host's own act where the host runs.
    pub machine: Option<MachineName>,
    /// What the World reads it as doing, which says how to show it.
    pub does: Does,
    /// Its arguments, as the host reported them.
    pub input: serde_json::Value,
    /// What it gave back, in order; empty while it runs.
    pub output: Vec<Content>,
    pub status: ToolStatus,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "as")]
pub enum Does {
    /// Runs a shell script. `exit` and `job` are what its answer says: how
    /// it exited, or the job it goes on as in the background.
    Command { command: String, cwd: Option<String>, exit: Option<i64>, job: Option<String> },
    /// Changes files.
    Edit { changes: Vec<Change> },
    /// Searches the web.
    Search { query: String },
    /// Looks at an image, or makes one.
    Image { path: Option<String> },
    /// Asks one of the World's colleagues in, or tells one something.
    Colleague { to: Option<String>, task: Option<String> },
    /// Starts one of the host's own agents, speaks to some, waits for some
    /// or stops them: Claude Code's Agent tool, Codex's agent tools. `agents`
    /// are the host's ids of those it concerns, as far as the call tells
    /// them; which call started an agent is told by the agent itself
    /// (`SessionChange::AgentStarted`).
    Agent {
        act: AgentAct,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        agents: Vec<String>,
        /// The kind of agent it starts, as the host names it.
        kind: Option<String>,
        /// The few words of what it is asked, or the words it is sent.
        task: Option<String>,
    },
    /// Anything else: its name, input and output say what it did.
    Call,
}

/// What a call does with the host's own agents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAct {
    Start,
    Message,
    Resume,
    Wait,
    Stop,
    List,
}

/// One file a tool changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub path: String,
    pub how: ChangeHow,
    /// The change as diff lines: `+` added, `-` removed, ` ` kept, `@@` a
    /// new place in the file.
    pub diff: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeHow {
    Added,
    Deleted,
    Updated,
    /// Updated, and moved to `to`.
    Moved {
        to: String,
    },
    /// Written whole, over whatever it held.
    Replaced,
}

/// A piece of what a tool gave back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Content {
    Text {
        text: String,
    },
    Image {
        mime: String,
        base64: String,
    },
    ImageUrl {
        url: String,
    },
    /// An image, kept where the session lives and fetched when it is shown;
    /// `content` names its bytes, so a viewer fetches them once.
    ImageRef {
        mime: String,
        content: Fingerprint,
        source: Box<crate::attachments::AttachmentSource>,
    },
    AudioUrl {
        url: String,
    },
    /// MCP content beyond plain text and base64 images, including resource
    /// links, embedded resources, audio, and annotations on ordinary blocks.
    Other {
        block: serde_json::Value,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Done,
    Failed,
    /// Refused before it ran, by the host or whoever answers its approvals.
    Declined,
    /// Its turn ended while it ran.
    Cut,
    Unknown,
}

/// One step of the agent's plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub text: String,
    pub status: StepStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The host's present-tense label while this step is in progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
    /// The host's complete task record, including fields not yet rendered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Doing,
    Done,
}

/// How a turn ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnEnd {
    pub how: TurnHow,
    /// Why it did not complete, in the host's words where it gave them.
    pub reason: Option<String>,
    /// Structured provider error, when the host supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<serde_json::Value>,
    pub duration_ms: Option<u64>,
    /// Observations belonging to this turn, never filled from user choices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<Box<TurnFacts>>,
    /// The agent's last message in the turn, verbatim: what goes back to
    /// whoever is owed the turn's end.
    pub last_message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnHow {
    Completed,
    Failed,
    /// Stopped before it finished: by whoever may stop it, or because the
    /// host's process or the node stopped under it.
    Interrupted,
    /// A word the host used that the World does not know, as it was.
    #[serde(untagged)]
    Other(String),
}

impl std::fmt::Display for TurnHow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TurnHow::Completed => "completed",
            TurnHow::Failed => "failed",
            TurnHow::Interrupted => "interrupted",
            TurnHow::Other(word) => word,
        })
    }
}

/// A fact about a session itself, as its host reported it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub kind: HealthKind,
    /// What happened, in a sentence, in the host's words where it gave them.
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthKind {
    /// The host is reconnecting to the model; the turn goes on.
    Reconnecting,
    /// The model's provider is holding the turn's output for extra safety
    /// processing.
    Safety,
    /// The model's provider moved the turn to another model.
    Rerouted,
    /// The World's tools are not there in this session: a session the World
    /// started has no other hands.
    ToolsMissing,
    /// Another tool server of the session did not start.
    ToolServer,
    Warning,
    /// An error the host reported that ended no turn.
    Error,
}

impl HealthKind {
    /// Whether whoever is owed the running turn's end must hear it at once:
    /// the session cannot do its work, or the model's provider stepped into
    /// the turn, which often comes before it is stopped.
    pub fn urgent(self) -> bool {
        matches!(self, HealthKind::ToolsMissing | HealthKind::Safety | HealthKind::Rerouted)
    }
}
