//! What local clients and the node say to each other.
//!
//! A client connects to the node's endpoint, says who it is, and is welcomed
//! or refused. After that it sends calls, one at a time, each with an id it
//! chose, and the node answers each call once. A client with several calls
//! to make at once opens several connections; negotiated WatchMany subscriptions
//! share one dedicated observation connection. Control frames are these
//! types in their JSON form. A file-read chunk header is followed by one
//! binary payload frame, as described by `file_read::FileReadPart`; an
//! Upload's dedicated connection carries raw byte frames through an empty
//! end frame, then the node sends its final JSON reply.

use serde::{Deserialize, Serialize};

use crate::residents::Host;
use crate::{Address, JobId, MachineName};

/// Raised whenever a change would make an older client or node misread a
/// frame, including a nested reply type. Tool adapters and job supervisors
/// already running keep the protocol they started with across a node
/// upgrade, so the node answers each in the shape it reads
/// (`fastctx-node`'s `renewal_legacy`); other mismatched clients are refused
/// before any call executes. `scripts/check-ipc-contract.py` decodes today's
/// replies with the readers of the protocols released so far.
pub const PROTOCOL: u32 = 13;

/// The generation of notices this build reads beyond those protocol 13
/// shipped with: 1 added a sentinel's finding, and the World's facts of a
/// delivery that failed and of a sentinel that woke. A client that says
/// none is sent each later notice in a shape it reads (`fastctx-node`'s
/// `renewal_legacy::notices`).
pub const NOTICES: u32 = 1;

/// The first frame on every connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    /// The client binary's version, shown when versions differ.
    pub version: String,
    pub peer: Peer,
    /// The generation of notices the client reads (`NOTICES`).
    #[serde(default)]
    pub notices: u32,
}

/// What is on the other end of a connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Peer {
    /// A command within an approved page job; it never speaks as the user.
    Page {
        page: Box<crate::pages::PageSource>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        job: Option<JobId>,
    },
    /// A command such as `fastctx w`, typed by a person or run by a
    /// resident; `resident` is set when a resident's `run` started it, and
    /// `session` when an agent's own shell did.
    Command {
        resident: Option<Address>,
        #[serde(default)]
        session: Option<SessionHint>,
    },
    /// The supervisor of one job, reporting on it.
    Supervisor { job: JobId },
    /// The MCP adapter of one agent session.
    Session { host: Host },
}

/// The agent session whose own shell ran a command, from what its host puts
/// in the environment of every command it runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SessionHint {
    /// `CODEX_THREAD_ID`.
    Codex { thread: String },
    /// The address in `CLAUDE_CODE_MESSAGING_SOCKET`; the token beside it is
    /// never read.
    Claude { inbox: String },
}

/// The node's answer to `Hello`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Welcome {
    Accepted {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        goal_runtime: bool,
        /// Several conversation subscriptions share a dedicated IPC connection.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        watch_many: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        goal_commands: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        profiles: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        account_records: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        conversation_plans: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        scratch_locations: bool,
        /// An older node says nothing of sentinels, and keeps none.
        #[serde(default)]
        sentinels: bool,
        machine: MachineName,
        version: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node_lifetime: Option<Box<crate::node_lifetime::NodeLifetime>>,
    },
    Refused {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mismatch: Option<crate::renewal::Mismatch>,
    },
}

/// A frame from a client after the welcome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
// Calls retain their existing allocation-free representation on ordinary connections.
#[allow(clippy::large_enum_variant)]
pub enum ClientFrame {
    McpLine { line: usize },
    Call { id: u64, call: crate::Call },
}

/// A frame from the node after the welcome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum NodeFrame {
    AccountRecords {
        id: u64,
        event: crate::account_records::Watched,
    },
    McpOpened {
        id: u64,
    },
    McpLine {
        id: u64,
        line: usize,
    },
    /// The one answer to a call; nothing more comes for its id.
    Reply {
        id: u64,
        reply: Result<crate::Reply, Failure>,
    },
    /// Part of what a `Watch` call hands over, which goes on until the
    /// client closes the connection or a `Reply` ends it.
    Watched {
        id: u64,
        watched: crate::session::Watched,
    },
    /// A byte-read header; see `FileReadPart` for the following binary frame.
    FileRead {
        id: u64,
        part: crate::file_read::FileReadPart,
    },
    /// A bounded UTF-8 piece of a logical row, followed by ReadComplete.
    LinesRead {
        id: u64,
        part: crate::lines::Part,
    },
    /// Part of what an `On` call hands over, until its `Reply`.
    On {
        id: u64,
        piece: crate::on::OnPiece,
    },
    /// The tools a `WatchTools` session is offered may have changed.
    ToolsChanged {
        id: u64,
    },
    /// The interface's optional parts as a `WatchFeatures` call sees them:
    /// first as they are, then after each change.
    Features {
        id: u64,
        features: crate::features::Features,
    },
    /// The World as a `WatchWorld` call sees it: first as it is, then after
    /// each change.
    World {
        id: u64,
        world: Box<crate::world::WorldNow>,
    },
}

/// Why a call did not do what it asked, in words for whoever reads the
/// answer: a person at a terminal or a model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct Failure {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<FailureCode>,
    /// A classified authorization refusal; old peers still read `code: refused`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denial: Option<crate::authorization::DenialReason>,
    /// The request failed before execution became possible. Absence means
    /// the caller must not infer that nothing ran.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub not_run: bool,
    /// Resource admission failures, independent of authorization and host errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity: Option<CapacityReason>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum CapacityReason {
    Settling { unfinished: usize },
    Pending,
    Waiting { seconds: u64 },
    Expired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    OriginalGone,
    /// The job is absent; this does not distinguish expiry from earlier collection.
    OutputNotRetained,
    /// The holding machine observed that the requested source does not exist.
    SourceMissing,
    /// No answer came from the machine that was to do it: it was not
    /// reached, or its answer was lost on the way. It may have been done;
    /// asked again under the same id, it is done once
    /// (`world-network.md`, 断开不是结束).
    Unanswered,
    /// The conversation asked about is not on the machine that was its
    /// home: it was deleted there. Asking again will not find it.
    Deleted,
    /// The machine asked refused the caller's account: asking again the
    /// same way is refused the same way until that account's rights there
    /// change.
    Refused,
    /// macOS keeps the place private from Fastctx on that machine: its user
    /// allows Fastctx in System Settings there (`world-gui.md`, 设置).
    MacPrivacy,
    /// A control's failure, told in its conversation already
    /// (`Offers::controls_told`): the conversation says it.
    Told,
    /// A host control may have executed, but its result could not be established.
    ControlUncertain,
    /// The requested action has not entered the host; the node retains it for retry.
    HostUnavailable,
    /// A code of a newer version, which this one does not know.
    #[serde(other)]
    Unknown,
}

impl Failure {
    pub fn capacity(mut self, reason: CapacityReason) -> Self {
        self.capacity = Some(reason);
        self
    }

    pub fn new(message: impl Into<String>) -> Self {
        Self { capacity: None, denial: None, message: message.into(), not_run: false, code: None }
    }

    pub fn control_uncertain(message: impl Into<String>) -> Self {
        Self {
            capacity: None,
            denial: None,
            message: message.into(),
            not_run: false,
            code: Some(FailureCode::ControlUncertain),
        }
    }

    pub fn is_control_uncertain(&self) -> bool {
        self.code == Some(FailureCode::ControlUncertain)
    }

    pub fn host_unavailable(message: impl Into<String>) -> Self {
        Self {
            capacity: None,
            denial: None,
            message: message.into(),
            not_run: true,
            code: Some(FailureCode::HostUnavailable),
        }
    }

    pub fn not_run(message: impl Into<String>) -> Self {
        Self { capacity: None, denial: None, message: message.into(), not_run: true, code: None }
    }

    pub fn original_gone() -> Self {
        Self {
            denial: None,
            capacity: None,
            message: "the original is gone and its host record holds no recoverable copy".into(),
            not_run: false,
            code: Some(FailureCode::OriginalGone),
        }
    }

    /// A place macOS keeps private from Fastctx, refused before anything ran.
    pub fn mac_privacy(message: impl Into<String>) -> Self {
        Self {
            capacity: None,
            denial: None,
            message: message.into(),
            not_run: true,
            code: Some(FailureCode::MacPrivacy),
        }
    }

    pub fn output_not_retained(message: impl Into<String>) -> Self {
        Self {
            capacity: None,
            denial: None,
            message: message.into(),
            not_run: false,
            code: Some(FailureCode::OutputNotRetained),
        }
    }

    pub fn source_missing(message: impl Into<String>) -> Self {
        Self {
            capacity: None,
            denial: None,
            message: message.into(),
            not_run: false,
            code: Some(FailureCode::SourceMissing),
        }
    }

    pub fn unanswered(message: impl Into<String>) -> Self {
        Self {
            capacity: None,
            denial: None,
            message: message.into(),
            not_run: false,
            code: Some(FailureCode::Unanswered),
        }
    }

    /// Whether no answer came: worth asking again, as it was asked.
    pub fn is_unanswered(&self) -> bool {
        self.code == Some(FailureCode::Unanswered)
    }

    /// Whether the machine had no place for it yet and did not start it:
    /// asking again a while later, as it was asked, is how it goes on.
    pub fn is_busy(&self) -> bool {
        self.not_run && self.capacity.is_some()
    }

    pub fn deleted(message: impl Into<String>) -> Self {
        Self { capacity: None, denial: None, message: message.into(), not_run: true, code: Some(FailureCode::Deleted) }
    }

    /// Whether the conversation asked about was deleted: nothing more will
    /// come of it.
    pub fn is_deleted(&self) -> bool {
        self.code == Some(FailureCode::Deleted)
    }

    pub fn refused(message: impl Into<String>) -> Self {
        Self { capacity: None, denial: None, message: message.into(), not_run: true, code: Some(FailureCode::Refused) }
    }

    /// Whether the machine refused the caller's account: asking again on
    /// a timer will not change that.
    pub fn is_refused(&self) -> bool {
        self.code == Some(FailureCode::Refused)
    }
}
