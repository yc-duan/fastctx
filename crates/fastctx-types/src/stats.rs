//! What a machine says of itself for its details page (`world-gui.md`, 机器的
//! 详情): when it started, how busy it is, its memory, disks, network cards
//! and graphics cards, and how its lines to its hubs have held. Only an
//! account that may work on the machine is told (`RemoteCall::Stats`).
//!
//! Rates (how busy the processors are, how fast the cards move bytes) are
//! measured between two readings: the machine's previous answer to anyone
//! who asked, or, when there was none lately, a second reading made a
//! moment after the first.
//!
//! The lines' history is long (an hour of round trips, thirty days of
//! changes), so one who asked before asks for only what came after its
//! last answer (`RemoteCall::Stats::after`) and keeps the rest.

use std::net::IpAddr;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{HubName, MachineName};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MachineStats {
    /// Which run of its node told it, for `StatsSeen`; an older node says nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    pub machine: MachineName,
    /// When this was read.
    pub at: Timestamp,
    /// When the machine started, as its clock has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot: Option<Timestamp>,
    /// The system as people call it, such as `Windows 11 Pro 24H2 (26200)`
    /// or `Ubuntu 24.04.1 LTS`.
    pub system: String,
    /// The kernel's own version, where it is not in `system`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel: Option<String>,
    /// The machine's own host name.
    pub host: String,
    pub arch: String,
    pub cpu: Cpu,
    pub memory: Memory,
    #[serde(default)]
    pub disks: Vec<Disk>,
    #[serde(default)]
    pub cards: Vec<Card>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gpus: Vec<Gpu>,
    /// Its lines to the hubs it is on.
    #[serde(default)]
    pub lines: Vec<Line>,
    /// Its node: the program's version, and when this node started.
    pub node: NodeSeen,
    /// The jobs running on it now, by the account each works for, with the
    /// limit its owner set for that account; its own under none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<AccountJobs>,
    /// How far it is toward its system's limits, a reading a minute over the
    /// last day, oldest first; with `after`, only the readings since.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<LimitReading>,
}

/// One reading of how far a machine is toward its system's limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitReading {
    pub at: Timestamp,
    pub uses: Vec<LimitUse>,
}

/// One limit and how much of it is used: by the user its node runs as, or
/// for `OpenFiles`, by the node itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitUse {
    pub kind: LimitKind,
    pub used: u64,
    pub limit: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitKind {
    InotifyInstances,
    InotifyWatches,
    OpenFiles,
    Tasks,
    /// One a newer node tells.
    #[serde(other)]
    Other,
}

/// How many jobs one account runs on a machine now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountJobs {
    /// None for the machine's own, its owner's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<crate::access::HubAccount>,
    pub running: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub most: Option<u16>,
}

/// The rates and shares in a reading are measured, never NaN, so a reading
/// equals itself as the replies holding it require.
impl Eq for MachineStats {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cpu {
    /// The processor's own name, such as `AMD Ryzen 9 7950X 16-Core
    /// Processor`; empty when the system does not say.
    pub model: String,
    /// Logical processors.
    pub threads: u32,
    /// How busy all of them were between the two readings, 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy: Option<f32>,
    /// The load averages over 1, 5 and 15 minutes, where the system keeps
    /// them (not Windows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load: Option<[f32; 3]>,
}

/// Bytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Memory {
    pub total: u64,
    pub used: u64,
    /// The page file or swap: what is set aside on disk, and in use.
    #[serde(default)]
    pub swap_total: u64,
    #[serde(default)]
    pub swap_used: u64,
    /// What the machine keeps free for its agents: a new turn waits while
    /// turns run and less is available (`world-lifecycle.md`, 内存). None
    /// where its node does not keep any, an older one; zero where it was
    /// turned off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<u64>,
    /// How many turns wait for that room now.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub waiting: u32,
}

fn is_zero(count: &u32) -> bool {
    *count == 0
}

/// The default reserve's ceiling. The node selects a supported reserve no
/// larger than half the machine's physical memory when no one chose one.
pub const MEMORY_ROOM: u64 = 1536 << 20;

/// What the user chooses from for it; zero keeps none.
pub const MEMORY_ROOMS: [u64; 8] = [0, 256 << 20, 512 << 20, 768 << 20, 1 << 30, 1536 << 20, 2 << 30, 3 << 30];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Disk {
    /// Where it is reached: `C:/`, or a mount point such as `/` or `/data`.
    pub at: String,
    /// Its label, where it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Its file system, such as `NTFS` or `ext4`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fs: String,
    pub total: u64,
    /// Bytes free to the machine's user.
    pub free: u64,
}

/// A network card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Card {
    pub name: String,
    pub up: bool,
    /// A card of the machine's own, not one a program made (a virtual
    /// switch, a tunnel): what crosses a tunnel crosses a card too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hardware: bool,
    /// Its link speed, in bits a second, where the system knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<u64>,
    /// Bytes received and sent since it came up.
    pub received: u64,
    pub sent: u64,
    /// Bytes a second, between the two readings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiving: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sending: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<IpAddr>,
}

/// A graphics card, as its driver's tool tells.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gpu {
    pub name: String,
    /// Bytes.
    pub memory_total: u64,
    pub memory_used: u64,
    /// How busy it is, 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy: Option<f32>,
}

/// The machine's line to one hub.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub hub: HubName,
    /// How many round trips and changes its node has kept in this run, the
    /// first of the next reading's to be numbered after them; an older
    /// node says nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<LineKept>,
    /// Since when it is up, or, while it is down, since when it tries.
    pub since: Timestamp,
    pub up: bool,
    /// The machine it goes through, when its own line is down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through: Option<String>,
    /// The round trips to the hub over the last hour, in milliseconds, one
    /// each heartbeat; only those after the moment asked for, when one was.
    #[serde(default)]
    pub trips: Vec<Trip>,
    /// When it came up and went down over the last 30 days, oldest first;
    /// only those after the moment asked for, when one was.
    #[serde(default)]
    pub changes: Vec<LineChange>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trip {
    pub at: Timestamp,
    pub ms: u32,
}

/// One change of a line to a hub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum LineChange {
    /// The node started: whatever was before it, the machine or the node
    /// was not running, or no one recorded it.
    Started {
        at: Timestamp,
    },
    Up {
        at: Timestamp,
    },
    Down {
        at: Timestamp,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        why: String,
    },
}

impl LineChange {
    pub fn at(&self) -> Timestamp {
        match self {
            LineChange::Started { at } | LineChange::Up { at } | LineChange::Down { at, .. } => *at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeSeen {
    pub version: String,
    pub started: Timestamp,
    /// Whether a system service keeps it running.
    #[serde(default)]
    pub service: bool,
    /// What of its work has gone on too long, as it last looked at itself
    /// (`world-concurrency.md`, 节点看着自己).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stuck: Vec<StuckWork>,
    /// Persistent data damage and current failures requiring attention.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub health: Vec<crate::session::Health>,
    /// The last place macOS kept private from it while it runs, and when
    /// (`world-gui.md`, 设置).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<Refused>,
}

/// One call admitted by the local node, without its user's request contents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallWork {
    pub id: u64,
    pub account: String,
    pub machine: String,
    pub conversation: Option<String>,
    pub kind: String,
    pub pool: String,
    pub phase: String,
    pub seconds: u64,
    pub within: Option<u64>,
    pub overdue: bool,
    pub waiting: String,
}

/// A place macOS kept private from a node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refused {
    pub path: String,
    pub at: Timestamp,
}

/// One piece of a node's work that has gone on too long.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StuckWork {
    /// What it was doing, as the node says it.
    pub doing: String,
    /// What it waits for or holds.
    pub what: String,
    /// For how long, in seconds.
    pub seconds: u64,
}

/// How many round trips and changes of a line its node has kept in one run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineKept {
    pub trips: u64,
    pub changes: u64,
}

/// What a reading of a machine's lines was told already: the run of its
/// node, and how many of each line's round trips and changes. The next
/// reading brings only those after them, whatever its clock did meanwhile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsSeen {
    pub run: String,
    pub lines: Vec<(HubName, LineKept)>,
}
