//! Residents: who an agent session is in the World, and its records
//! (`world-town.md`, 居民目录).

use std::collections::BTreeMap;

mod about;
mod question;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::run::{EndedHow, ExitStatus};
use crate::session::{Health, Speaker, TurnEnd};
use crate::{Address, AgentName, JobId, MachineName, NoticeId, ResidentId, TownName};

/// The program a session runs in, as the session itself reported it in the
/// MCP handshake.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub agent: AgentName,
    /// The client name from the MCP handshake, such as `codex-mcp-client`.
    pub client: String,
    pub version: String,
}

/// What a session knows about itself that lets the World recognise it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hints {
    /// The native Codex login that owns the thread's persisted history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<std::path::PathBuf>,
    /// OS lifetime of the native host that started this adapter, reported locally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_process: Option<crate::ProcessIdentity>,
    /// Set by the World when it started the session itself.
    pub resident: Option<ResidentId>,
    /// The thread a Codex call belongs to; one thread is one resident,
    /// whichever connections it uses.
    pub thread: Option<String>,
    /// The resident this session arrived as before, when its connection to
    /// the node broke and it arrives again, as after the node restarted.
    #[serde(default)]
    pub was: Option<ResidentId>,
    /// The Claude tool call's `_meta["claudecode/toolUseId"]`. Its mod
    /// publishes the same id before allowing this particular call through.
    #[serde(default)]
    pub tool_use_id: Option<String>,
}

/// A resident's `about` record: who it is and how it came to be, written
/// once when it is born.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "about::Stored")]
pub struct About {
    pub id: ResidentId,
    pub host: Host,
    pub machine: MachineName,
    pub born_at: Timestamp,
    pub origin: Origin,
    /// The conversation this one was forked from, even after its source is deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<Address>,
    /// The session or thread id on the host's side, when there is one.
    pub session: Option<String>,
    /// The World started the session and reaches it itself: a thread of
    /// this machine's Codex app-server, or a Claude Code background session.
    /// Any other session is the user's own, which the World only hears
    /// through its calls of the World's tools.
    #[serde(default)]
    pub world_started: bool,
    /// A native Codex thread the World resumed for a requested background event.
    /// Birth provenance remains unchanged in world_started and origin.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub native_resumed: bool,
    /// The model, as the host told the World; `None` when it did not.
    #[serde(default)]
    pub model: Option<String>,
    /// Whom it works for.
    #[serde(default)]
    pub acting: crate::access::Acting,
    /// The machines and folders the user points it to.
    #[serde(default, skip_serializing_if = "crate::focus::Focus::is_empty")]
    pub focus: crate::focus::Focus,
    /// The MCP servers of the account it works for that it starts without
    /// (`world-user-config.md`).
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub mcp_off: std::collections::BTreeSet<String>,
    /// The agent's home it was born in, its account on this machine
    /// (`agents::AgentHome`), when not the default one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<std::path::PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callee: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Origin {
    /// The user opened the session.
    User,
    /// Another resident asked for it, with this label for the task.
    Colleague { asked_by: Address, label: String },
}

/// Something the World tells a resident: handed to its host when the host
/// gives the World a way in, otherwise waiting in its inbox until its next
/// call of a World tool. The id names it on the resident's machine and in
/// the letter that carries it from another.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub id: NoticeId,
    pub at: Timestamp,
    pub notice: Notice,
}

/// The last lines of a job's output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastLines {
    pub text: String,
    /// Whether the text is all of the output.
    pub whole: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Notice {
    /// A requested observation, allowed to resume the owner's closed session.
    Sentinel { observation: Box<crate::sentinel_notice::SentinelNotice> },
    /// A job the resident started, which went on in the background, ended.
    JobEnded {
        /// A wait took this end; this is a receipt, never a session notification.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        taken: bool,
        job: JobId,
        machine: MachineName,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<NoticeId>,
        /// Where this hop's CLI took the end, for arbitration of nested calls.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        follower: Option<MachineName>,
        /// Completion ids already included in this job's enclosing output.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        consumed: Vec<NoticeId>,
        /// The receiver released this end without a conclusive follower answer.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        follower_unconfirmed: bool,
        how: EndedHow,
        /// How the shell itself exited, when it did.
        status: Option<ExitStatus>,
        at: Timestamp,
        /// The log holding all of its output, on that machine.
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stderr: Option<String>,
        /// The end of that output as a terminal shows it, so the resident
        /// knows how it went without reading the log.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last: Option<LastLines>,
        /// Who stopped it and why, when someone did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop: Option<Box<crate::run::StopRequest>>,
        /// Coordinates learned after the initial run answer, including nested hops.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        scratch: Vec<crate::scratch::Location>,
    },
    /// A turn of a colleague ended, which the resident's words went into or
    /// which it asked for: its last message, verbatim.
    Answer { colleague: Address, label: String, turn: String, end: TurnEnd },
    /// A colleague's host reported something in a turn owed to the resident
    /// that it must hear at once.
    Health { colleague: Address, label: String, health: Health },
    /// A stored question from before questions used ordinary resident messages.
    /// Reading preserves its identity; writing emits the compatible Words shape.
    #[serde(rename(serialize = "words", deserialize = "question"), serialize_with = "question::serialize")]
    Question { colleague: Address, id: String, text: String },
    /// Words someone said to the resident.
    Words {
        from: Speaker,
        text: String,
        /// A goal conclusion keeps its sender and full text for older readers.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal_report: Option<Box<crate::goals::ReportDelivery>>,
        /// A nonblocking question retains its sender and full text for older readers.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal_question: Option<Box<crate::goals::QuestionDelivery>>,
    },
    /// The user's words and files; the accompanying facts are the World's.
    Attached { message: crate::session::attachments::AttachedMessage, text: String },
    /// A fact of the World's own, with the words the resident reads for it.
    World { fact: WorldFact, text: String },
}

impl Notice {
    /// Whether this notice carries a colleague's nonblocking question.
    pub fn is_question(&self) -> bool {
        matches!(self, Self::Question { .. } | Self::Words { goal_question: Some(_), .. })
    }

    /// The account frozen with goal-derived content, when its record retained one.
    pub fn goal_account(&self) -> Option<&crate::access::HubAccount> {
        match self {
            Self::Words { goal_report: Some(report), .. } => Some(&report.account),
            Self::Words { goal_question: Some(question), .. } => question.account.as_ref(),
            _ => None,
        }
    }

    /// Scheduled work and requested answers resume even a closed foreground conversation.
    pub fn wakes_closed(&self) -> bool {
        !self.is_question()
            && matches!(
                self,
                Self::Words { from: Speaker::World, .. }
                    | Self::Words { goal_report: Some(_), .. }
                    | Self::Answer { .. }
                    | Self::Sentinel { .. }
            )
    }

    /// What the World tells when the user changes a conversation's machines
    /// and folders, or its project: it starts no turn, but goes in with the
    /// next one, or into the one running where it next can; told again
    /// before it went in, only the latest goes (`world-colleagues.md`).
    /// Notices of one kind replace one another.
    pub fn along(&self) -> Option<&str> {
        match self {
            Notice::World { fact: WorldFact::Focus { .. }, .. } => Some("focus"),
            Notice::World { fact: WorldFact::Moved { .. }, .. } => Some("project"),
            Notice::Question { id, .. } => Some(id),
            Notice::Words { goal_question: Some(question), .. } => Some(&question.id),
            _ => None,
        }
    }
}

/// What the World tells a resident of its surroundings, as facts an
/// interface can show without reading the words given to the model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "fact")]
pub enum WorldFact {
    /// A durable delivery reached a terminal failure, with its explanation in the notice.
    DeliveryFailed { id: NoticeId },
    /// Another session holds this machine's Computer Use in its turn.
    DeskTaken { by: Address },
    /// A session the World does not know holds this machine's Computer Use.
    DeskOutside,
    /// The turn holding this machine's Computer Use ended.
    DeskFree { by: Address },
    /// The user moved the resident into a project, or out of its project.
    Moved {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<crate::TownName>,
    },
    /// The user changed the machines and folders it is pointed to.
    Focus { focus: crate::focus::Focus },
}

/// Who stops a job when the user does, from an interface: the job's end is
/// told to its resident as stopped by the user.
pub const STOPPED_BY_USER: &str = "the user";

/// How words for a resident landed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Landed {
    /// Submission crossed a host boundary without an acceptance result.
    /// Its exact words remain recorded, and are not submitted again.
    Uncertain { why: String },
    /// It was idle: a new turn started with them.
    NewTurn { turn: String },
    /// It was working: they join its running turn at its next tool boundary.
    JoinedTurn { turn: String },
    /// Its host gives the World no way in now: they wait for its next call
    /// of a World tool, or for it to come back. `why` is set when the World
    /// has a way in and it failed. `answered` says whether the end of the
    /// turn they go into will be told: the World sees the turns of the
    /// sessions it holds and of Claude sessions with its plugin, no others.
    Waiting {
        why: Option<String>,
        #[serde(default)]
        answered: bool,
    },
    /// It lives on another machine: they went there as a letter, which that
    /// machine hands over when it has it.
    Sent,
    /// Handed to the World's plugin in its session, which brings them in at
    /// once when it is idle and at its next tool boundary when it is busy.
    Handed { id: NoticeId },
}

/// What a resident is doing now, and what that commits the World to: its
/// `now`, rewritten whenever it changes. Kept for residents whose host
/// tells the World when they work: Codex threads the World holds, and
/// Claude sessions through the World's mod.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Now {
    pub activity: Activity,
    pub since: Timestamp,
    /// The turn running now, as its host named it.
    pub turn: Option<String>,
    /// The latest settled turn; late settlement of an older turn cannot replace it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn: Option<String>,
    /// For each turn running, whose words went into it: each is owed its
    /// end. Once a turn ends, its answers wait in the `outbox` until each
    /// reader's machine has its own.
    #[serde(default)]
    pub owed: BTreeMap<String, Vec<Address>>,
    /// Who the latest completed turn was for. A turn the World
    /// starts with a fact of the resident's own work, such as a job's end,
    /// is owed to them: they are who it is working for.
    pub answering: Vec<Address>,
    /// The latest thing that went wrong: what its host reported about the
    /// session, or how its last turn ended when it did not complete.
    pub trouble: Option<Trouble>,
    /// It sits at this machine's Computer Use desk until its turn ends.
    #[serde(default)]
    pub desk: bool,
    /// When the World first confirmed this turn holds the desk.
    #[serde(default)]
    pub desk_since: Option<Timestamp>,
    /// The running turn cannot continue until a user answers its host.
    #[serde(default)]
    pub waiting_for_user: bool,
    #[serde(default)]
    pub waiting_since: Option<Timestamp>,
    /// The conversation's name, as its host last gave it.
    #[serde(default)]
    pub title: Option<String>,
    /// Latest tool start observed from the host, even when it is not a World tool.
    #[serde(default)]
    pub last_tool_at: Option<Timestamp>,
    /// The start of the first words the user said here, which name the
    /// conversation where its host gave it no name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_words: Option<String>,
    /// The start of the last message of the latest turn that ended: what a
    /// notification of that turn says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_said: Option<String>,
    /// The latest turn that ended compacted the context and said nothing
    /// after it: what a notification of that turn says.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub last_compacted: bool,
    /// Whether the latest turn that ended took words the user sent: its end
    /// stands for an answer it gave without `message_user`, while a turn the
    /// World started that said nothing tells the user nothing
    /// (`world-voice.md`). None from a node that does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heard_user: Option<bool>,
    /// What the agent asked the user and has not had answered: what a
    /// notification of it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked: Option<Asked>,
    /// What the agent last told the user through `message_user`: what a
    /// notification of it and the conversation's line on its project's page
    /// say (`world-voice.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub told: Option<Told>,
}

/// How much of what the agent told the user `Told::text` keeps, in
/// characters.
pub const TOLD_CHARS: usize = 500;

/// Something the agent told the user through `message_user`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Told {
    /// New for each message: a new one is something new said.
    pub id: String,
    pub at: Timestamp,
    pub status: TellStatus,
    /// Its start, as the agent wrote it.
    pub text: String,
    /// The turn it was said in, as its host named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
}

/// Why the agent speaks, which decides how its words reach the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TellStatus {
    /// An answer to what the user just said.
    Reply,
    /// The agent's own, which can wait until the user looks.
    Info,
    /// The agent's own, which the user should know soon.
    Urgent,
}

/// How much of the user's first words `Now::first_words` keeps, in
/// characters: a name in a list shows fewer.
pub const FIRST_WORDS_CHARS: usize = 80;
/// How much of a turn's last message `Now::last_said` keeps, in characters:
/// a notification's body shows a few lines of it.
pub const LAST_SAID_CHARS: usize = 500;
/// How much of each question `Asked::text` keeps, in characters.
pub const ASKED_CHARS: usize = 300;

/// Something the agent asked the user: a request its turn waits on, or a
/// question it asked without waiting, which the user may answer whenever
/// they like or never.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asked {
    /// The request's reply address or the question's item, as a string: a
    /// new one is a new question.
    pub id: String,
    pub at: Timestamp,
    /// Whether the agent's turn waits for the answer.
    pub waits: bool,
    pub what: AskedFor,
    /// What it asks, as it said it: its questions, one a line; the command
    /// or the files it asks leave for; the tool's name; a form's message.
    pub text: String,
    /// The answers it offers, when it asks a single question with options.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

/// The line that opens the user's answers to a question Codex asked without
/// waiting, as Codex's own clients write them.
pub const ASYNC_REPLY_TAG: &str = "<send_user_message_question_reply>";

impl Asked {
    /// The user's words choosing option `choice` of a question asked without
    /// waiting, in Codex's envelope (`gui/src/conversation/async-ask.ts`):
    /// its id names the question's item and the question's place among its
    /// questions. None when it offers no such option.
    pub fn reply(&self, choice: usize) -> Option<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Answered<'a> {
            question_item_id: String,
            question: &'a str,
            answer: &'a str,
        }
        let answer = self.options.get(choice).filter(|_| !self.waits && self.what == AskedFor::Answer)?;
        let id = serde_json::to_string(&("request_user_input_async", &self.id, 0)).expect("strings serialize");
        let answered = [Answered { question_item_id: id, question: &self.text, answer }];
        let close = ASYNC_REPLY_TAG.replacen('<', "</", 1);
        Some(format!("{ASYNC_REPLY_TAG}\n{}\n{close}", serde_json::to_string(&answered).expect("strings serialize")))
    }
}

/// What an `Asked` wants of the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskedFor {
    /// Answers to its questions.
    Answer,
    /// Leave to run a command.
    Command,
    /// Leave to change files.
    Change,
    /// Leave to use a tool.
    Tool,
    /// Leave to carry out its plan.
    Plan,
    /// A form filled in, or a link opened, for one of its tools.
    Form,
    /// Something the interface cannot answer, such as wider permissions.
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activity {
    Busy,
    Idle,
    /// Not here: a colleague whose requester left, or a session that ended.
    /// Words wake it.
    Asleep,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trouble {
    pub at: Timestamp,
    pub text: String,
}

/// One line of a colleague's `messages`: what it was asked, what it was
/// told, how each turn ended and how its answers reached their readers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageLine {
    pub at: Timestamp,
    #[serde(flatten)]
    pub what: MessageKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum MessageKind {
    /// The request it was born with.
    Asked {
        from: Address,
        text: String,
        /// The requester's initiating turn, not the colleague's first turn.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requester_turn: Option<String>,
    },
    /// Words that went into one of its turns.
    Heard { from: Speaker, turn: Option<String>, text: String },
    /// A turn ended, owed to `owed`.
    Ended { turn: String, end: TurnEnd, owed: Vec<Address> },
    /// How word of a turn reached one who was owed it.
    Told { to: Address, turn: Option<String>, outcome: Outcome },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Outcome {
    /// Handed to its host now.
    Delivered,
    /// Waiting on its machine until it can be handed over; `why` is set
    /// when handing it over now failed.
    Waiting {
        why: Option<String>,
    },
    /// Sent as a letter to its machine.
    Sent,
    Failed {
        reason: String,
    },
}

/// What a resident saw through view. Text decoding validates and hashes the
/// complete byte stream even when only selected lines are displayed. Other
/// partial views can record size and modification time without a full hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    pub path: String,
    pub at: Timestamp,
    pub sha256: Option<String>,
    pub size: u64,
    pub modified: Option<Timestamp>,
    /// The encoding it was read in, for text.
    pub encoding: Option<String>,
    /// The user's explicit codec or per-line codec order; inferred labels are not overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding_selection: Option<String>,
}

/// One thing the World saw a resident do: one line of its `actions`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    /// The initiating conversation turn, captured before the work starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    pub at: Timestamp,
    /// Where it happened, which may be another machine than the resident's.
    pub machine: MachineName,
    /// The hand it used: `run`, `edit`, `write`, `apply_patch`, `view`.
    pub hand: String,
    /// What it did, in a line: the command's first line, the file written.
    pub what: String,
    /// How it came out: `exit 0`, `running as j-k3x9a2`, `3 edits`.
    pub result: String,
}

/// That a resident left a town, and for which (`fastctx w` lists it in the
/// town it left).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Moved {
    /// The town it went to; `None` for outside every town.
    pub to: Option<TownName>,
    pub at: Timestamp,
}

/// The projects a resident has been in, oldest first; the last is the one it
/// is in now. Kept in its `town`: changing projects changes this and nothing
/// else.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Places {
    pub stays: Vec<Stay>,
}

/// From `from` on, until the next stay, the resident was in `town`, or in
/// no project when it is `None`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stay {
    pub town: Option<TownName>,
    pub from: Timestamp,
}

impl Places {
    /// A resident born at `at` in `town`.
    pub fn born(town: Option<TownName>, at: Timestamp) -> Self {
        Self { stays: vec![Stay { town, from: at }] }
    }

    /// The project it is in now.
    pub fn now(&self) -> Option<&TownName> {
        self.stays.last().and_then(|stay| stay.town.as_ref())
    }

    /// The project it was in at `at`; the first one for a moment before it.
    pub fn at(&self, at: Timestamp) -> Option<&TownName> {
        self.stays.iter().rev().find(|stay| stay.from <= at).or(self.stays.first()).and_then(|stay| stay.town.as_ref())
    }

    /// It is in `town` from `at` on. Returns whether that changed anything.
    pub fn enter(&mut self, town: Option<TownName>, at: Timestamp) -> bool {
        if self.stays.last().is_some_and(|stay| stay.town == town) {
            return false;
        }
        self.stays.push(Stay { town, from: at });
        true
    }

    /// Each time it left a project: the project, where it went and when.
    pub fn departures(&self) -> Vec<(Option<TownName>, Moved)> {
        self.stays
            .windows(2)
            .map(|pair| (pair[0].town.clone(), Moved { to: pair[1].town.clone(), at: pair[1].from }))
            .collect()
    }

    /// The stretches of time it was in `town`: from, and until when it left.
    pub fn spans(&self, town: Option<&TownName>) -> Vec<(Timestamp, Option<Timestamp>)> {
        self.stays
            .iter()
            .enumerate()
            .filter(|(_, stay)| stay.town.as_ref() == town)
            .map(|(index, stay)| (stay.from, self.stays.get(index + 1).map(|next| next.from)))
            .collect()
    }
}

/// Something on its way into a resident's session: one file in its
/// `inbox`, kept until the World knows which turn took it, so whoever is
/// owed that turn's end is owed it from then on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub delivery: Delivery,
    /// Who is owed the end of the turn it goes into.
    pub readers: Vec<Address>,
    pub state: PendingState,
    /// Resolve the audience when these words actually enter a turn, after
    /// any earlier submission has recorded its readers.
    #[serde(default)]
    pub working: bool,
    /// A Claude conversation already entrusted with these words. A later
    /// /clear must not move them into the replacement conversation.
    #[serde(default)]
    pub session: Option<String>,
    /// When they were last handed to the session's running host, or sent to
    /// wake it: the host activation that has them is the one running then,
    /// or the one that comes up after a wake. Records kept before this was
    /// written have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<jiff::Timestamp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingState {
    /// It waits for a way in: the next words that go into the session, the
    /// World's plugin in it, or its next call of a World tool.
    Waiting,
    /// It is being put into a Codex thread; the app-server's answer says
    /// which turn took it.
    Sending,
    /// It is being handed to the World's plugin in a Claude session. The
    /// plugin's receipt identifies the turn that takes it.
    Handed,
    /// A Claude session stopped by its supervisor was started again with it
    /// as its first words: the session's next turn takes it.
    Waking,
}

/// A turn's end on its way to one reader: kept from the moment the turn
/// ends until the reader's machine has it. One file in the `outbox` of the
/// resident whose turn it was.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owing {
    pub to: Address,
    pub delivery: Delivery,
}

/// Where a resident is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Whereabouts {
    pub resident: ResidentId,
    pub town: Option<TownName>,
}

impl About {
    /// Whether the World retains a restoration contract for this host session.
    pub fn world_managed(&self) -> bool {
        self.world_started || self.native_resumed
    }
}
