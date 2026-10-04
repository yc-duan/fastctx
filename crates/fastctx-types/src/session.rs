//! What happens in an agent's session, in one form for every host
//! (`world-colleagues.md`, 一轮怎样结束 and 用户在任何地方和居民说话).
//!
//! Each host reports its sessions in its own words: Codex's app-server in
//! notifications about threads, turns and items, Claude Code through the
//! World's mod. Where the World first hears them it translates them into
//! these events, and everything that follows a session (`fastctx talk`, the
//! TUI, a colleague's requester, later the graphical interface) reads only
//! these. A host's report that has no event here is not passed on, and
//! nothing here is made up where the host said nothing.
//!
//! A turn is one round of work: it starts when words go into an idle
//! session and ends when the agent stops. Words that arrive while a turn
//! runs join it at its next tool boundary.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{Address, MachineName};

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
    /// Words went into the session. `from` is unknown where the host's
    /// record of them does not say, as in a thread's history.
    Heard {
        turn: Option<String>,
        from: Option<Speaker>,
        text: String,
    },
    /// A piece of a message the agent is writing. Only those watching live
    /// see these; the whole message follows as `Said`.
    Writing {
        turn: String,
        item: String,
        delta: String,
    },
    /// A message the agent wrote, whole.
    Said {
        turn: String,
        item: String,
        text: String,
    },
    /// The agent started using a tool. `tool` names it as the host does,
    /// such as `fastctx.run` or `apply_patch`; `detail` says in a line what
    /// it was used on, when the World can tell.
    ToolStarted {
        turn: String,
        item: String,
        tool: String,
        detail: String,
    },
    ToolEnded {
        turn: String,
        item: String,
        tool: String,
        detail: String,
        failed: bool,
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
}

/// What watching a session hands over: first what happened before, as far
/// as its host keeps it, then each event as it happens.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Watched {
    /// What happened before; empty when the host keeps nothing the World
    /// can read.
    History {
        events: Vec<SessionEvent>,
    },
    Live {
        event: SessionEvent,
    },
}

/// Who said words that went into a session.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Speaker {
    /// The user, typing on `machine`.
    User { machine: MachineName },
    /// Another resident.
    Resident { address: Address },
    /// The World, telling a fact, such as that a job ended.
    World,
}

/// How a turn ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnEnd {
    pub how: TurnHow,
    /// Why it did not complete, in the host's words where it gave them.
    pub reason: Option<String>,
    pub duration_ms: Option<u64>,
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
