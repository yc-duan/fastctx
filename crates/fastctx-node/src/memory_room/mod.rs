//! What this machine keeps free in memory for its agents
//! (`world-lifecycle.md`, 内存). A turn grows as it works, and Claude Code's
//! daemon retires idle sessions, the one about to be woken among them, once
//! less than a gigabyte is available. So a new turn waits while turns run
//! here and less than the room is available, a turn let through and not yet
//! begun counted as about to take its share; it starts when one ends or
//! memory comes back. One turn always may: while none runs, none waits.
//! Turns wait for it in the order they came, each behind its login's own
//! limit first (`turn_slots`). On Linux the node also keeps the room
//! (`keeper`).

mod keeper;

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;
use fastctx_types::stats::{MEMORY_ROOM, MEMORY_ROOMS};
use serde::{Deserialize, Serialize};

use crate::Node;

pub(crate) use keeper::start;

/// How long a waiting turn keeps its place without asking again: its words
/// ask every few seconds, so one that stopped asking has gone another way.
const ASKING: Duration = Duration::from_secs(30);
/// How long a turn let through counts as about to grow before its host says
/// it began.
const STARTING: Duration = Duration::from_secs(120);
/// What a turn let through and not yet begun is taken to need.
const GROWTH: u64 = 512 << 20;
/// How long a new conversation's first turn waits for memory at most: its
/// opening is answered within five minutes.
const OPENING: Duration = Duration::from_secs(4 * 60);
/// How often it looks again meanwhile.
const LOOK_AGAIN: Duration = Duration::from_secs(2);

pub(crate) struct Room {
    bytes: AtomicU64,
    line: Mutex<Line>,
}

#[derive(Default)]
struct Line {
    /// The turns waiting, in the order they came, each with when it last asked.
    waiting: Vec<(ResidentId, Instant)>,
    /// Turns let through whose host has not said yet that they began.
    starting: Vec<(ResidentId, Instant)>,
}

/// A turn waiting for memory: how many wait ahead of it, what the machine
/// keeps free and what it has available.
pub(crate) struct Waits {
    pub ahead: u32,
    pub room: u64,
    pub available: u64,
}

#[derive(Serialize, Deserialize)]
struct Kept {
    bytes: u64,
}

impl Room {
    pub(crate) fn new(world: &WorldDir) -> Self {
        let bytes = std::fs::read(file(world))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Kept>(&bytes).ok())
            .map_or_else(default_room, |kept| kept.bytes);
        Self { bytes: AtomicU64::new(bytes), line: Mutex::default() }
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    /// How many turns wait for memory now.
    pub(crate) fn waiting(&self) -> u32 {
        let line = self.line.lock().expect("never poisoned");
        let now = Instant::now();
        line.waiting.iter().filter(|(_, asked)| now.duration_since(*asked) < ASKING).count() as u32
    }
}

/// Keep the usual reserve on larger machines without reserving nearly all of
/// a small machine. Explicit settings are preserved, including zero.
fn default_room() -> u64 {
    fastctx_platform::system::memory().map_or(MEMORY_ROOM, |memory| {
        MEMORY_ROOMS.iter().copied().filter(|room| *room <= MEMORY_ROOM && *room <= memory.total / 2).max().unwrap_or(0)
    })
}

fn file(world: &WorldDir) -> PathBuf {
    world.path().join("memory-room.json")
}

/// What `machine`, or this computer, keeps free; sets it first when `bytes`
/// is given.
pub(crate) async fn query(
    node: &std::sync::Arc<Node>,
    acting: &fastctx_types::access::Acting,
    machine: Option<fastctx_types::access::Written>,
    bytes: Option<u64>,
) -> Result<u64, Failure> {
    let Some(target) = machine.filter(|machine| !node.is_here(machine)) else { return answer(node, bytes) };
    match node
        .call(
            acting,
            &target,
            fastctx_types::access::Purpose::Hands,
            fastctx_types::remote::RemoteCall::MemoryRoom { bytes },
        )
        .await
        .map_err(|failure| Failure::new(failure.to_string()))??
    {
        fastctx_types::remote::RemoteReply::MemoryRoom { bytes } => Ok(bytes),
        _ => Err(Failure::new(format!("{target} answered something other than what it keeps free"))),
    }
}

/// What this machine keeps free; sets it first when `bytes` is given.
pub(crate) fn answer(node: &std::sync::Arc<Node>, bytes: Option<u64>) -> Result<u64, Failure> {
    let Some(bytes) = bytes else { return Ok(node.memory_room.bytes()) };
    if !MEMORY_ROOMS.contains(&bytes) {
        return Err(Failure::new(format!("{bytes} bytes is not a room the machine keeps")));
    }
    let written = serde_json::to_vec(&Kept { bytes }).map_err(|error| Failure::new(error.to_string()))?;
    fastctx_platform::publish(&file(&node.world), &written, fastctx_platform::Permissions::Private)
        .map_err(|error| Failure::new(format!("keeping what the machine keeps free: {error}")))?;
    node.memory_room.bytes.store(bytes, Ordering::Relaxed);
    node.log.line(format_args!("this machine keeps {} free for its agents now", size(bytes)));
    // Less kept may let the turns waiting go.
    let waiting: Vec<ResidentId> =
        node.memory_room.line.lock().expect("never poisoned").waiting.iter().map(|(id, _)| id.clone()).collect();
    wake(node, waiting);
    Ok(bytes)
}

/// Whether `id`, its login having room for it, may start its turn now as
/// far as memory goes, while the turns in `running` run here; else its place
/// in the line, kept while it asks again. Called under the login's lock.
pub(crate) fn admit(node: &Node, id: &ResidentId, running: &[ResidentId]) -> Result<(), Waits> {
    let room = node.memory_room.bytes();
    let mut line = node.memory_room.line.lock().expect("never poisoned");
    line.admit(id, running, room, available(), Instant::now())
}

/// Waits for memory for the first turn of `id`, a new conversation whose
/// session starts with it: as a turn of one already open would, but only
/// as long as its opening may take, and then it starts all the same.
pub(crate) async fn wait(node: &Node, id: &ResidentId) {
    let deadline = Instant::now() + OPENING;
    loop {
        let running = running(node, id);
        let room = node.memory_room.bytes();
        let gone = {
            let mut line = node.memory_room.line.lock().expect("never poisoned");
            let now = Instant::now();
            match line.admit(id, &running, room, available(), now) {
                Ok(()) => true,
                Err(_) if now >= deadline => {
                    line.go(id, now);
                    true
                }
                Err(_) => false,
            }
        };
        if gone {
            return;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The turns running here now, but `except`'s.
pub(crate) fn running(node: &Node, except: &ResidentId) -> Vec<ResidentId> {
    node.residents
        .everyone()
        .filter(|record| record.removed.is_none() && record.id != *except)
        .filter(|record| {
            record
                .now
                .as_ref()
                .is_some_and(|now| now.turn.is_some() || now.activity == fastctx_types::residents::Activity::Busy)
        })
        .map(|record| record.id)
        .collect()
}

impl Line {
    /// `admit` with what the machine keeps free and has `available` (none
    /// where it cannot be read, which holds nobody back) as of `now`.
    fn admit(
        &mut self,
        id: &ResidentId,
        running: &[ResidentId],
        room: u64,
        available: Option<u64>,
        now: Instant,
    ) -> Result<(), Waits> {
        self.waiting.retain(|(_, asked)| now.duration_since(*asked) < ASKING);
        self.starting.retain(|(started, at)| !running.contains(started) && now.duration_since(*at) < STARTING);
        if room == 0 {
            self.waiting.retain(|(waiting, _)| waiting != id);
            return Ok(());
        }
        match self.waiting.iter_mut().find(|(waiting, _)| waiting == id) {
            Some(waiting) => waiting.1 = now,
            None => self.waiting.push((id.clone(), now)),
        }
        let place = self.waiting.iter().position(|(waiting, _)| waiting == id).expect("it waits in line");
        let starting = self.starting.iter().filter(|(started, _)| started != id).count() as u64;
        let quiet = running.is_empty() && starting == 0;
        let enough = available.is_none_or(|available| available >= room + starting * GROWTH);
        if place == 0 && (quiet || enough) {
            self.go(id, now);
            return Ok(());
        }
        Err(Waits { ahead: place as u32, room, available: available.unwrap_or(0) })
    }

    /// `id` goes: out of the line, and counted as about to grow until its
    /// host says its turn began.
    fn go(&mut self, id: &ResidentId, now: Instant) {
        self.waiting.retain(|(waiting, _)| waiting != id);
        self.starting.retain(|(started, _)| started != id);
        self.starting.push((id.clone(), now));
    }
}

/// How many turns wait for memory ahead of `id`'s, while it waits.
pub(crate) fn ahead(node: &Node, id: &ResidentId) -> Option<u32> {
    let line = node.memory_room.line.lock().expect("never poisoned");
    let now = Instant::now();
    line.waiting
        .iter()
        .filter(|(_, asked)| now.duration_since(*asked) < ASKING)
        .position(|(waiting, _)| waiting == id)
        .map(|place| place as u32)
}

/// A turn here ended: the first turn waiting asks again now.
pub(crate) fn ended(node: &std::sync::Arc<Node>, id: &ResidentId) {
    let first = {
        let mut line = node.memory_room.line.lock().expect("never poisoned");
        line.starting.retain(|(started, _)| started != id);
        line.waiting.first().map(|(first, _)| first.clone())
    };
    wake(node, first.into_iter().collect());
}

/// The memory available here now, as the system counts it.
fn available() -> Option<u64> {
    fastctx_platform::system::memory().ok().map(|memory| memory.total.saturating_sub(memory.used))
}

fn wake(node: &std::sync::Arc<Node>, ids: Vec<ResidentId>) {
    for id in ids {
        crate::lifecycle::retry::kick(node, &id);
        crate::lifecycle::retry::changed(node, &id);
    }
}

/// Bytes as a person reads them, for the log and a waiting turn's reason.
pub(crate) fn size(bytes: u64) -> String {
    const GIB: f64 = (1u64 << 30) as f64;
    const MIB: f64 = (1u64 << 20) as f64;
    match bytes {
        bytes if bytes >= 1 << 30 => format!("{:.1} GiB", bytes as f64 / GIB),
        bytes => format!("{:.0} MiB", bytes as f64 / MIB),
    }
}
