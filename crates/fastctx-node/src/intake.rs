//! Admission of the calls this machine answers (`world-concurrency.md`, 别的
//! 机器来的调用按来者公平地分): equal-cost, two-level deficit round robin
//! within each kind, by account and then by conversation, execution limits
//! and settlement counts independent for each kind.
//!
//! A call waits its turn for as long as half its own time allows, the other
//! half left to do it. Nothing is refused for how many others wait: only an
//! account holding more of the machine's memory in calls than its share,
//! or the machine holding as much as it may, which no client working as it
//! should brings about, and then only that account's calls.

mod diagnostics;
mod occupancy;
mod queue;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use fastctx_net::ids::HubId;
use fastctx_net::link::admission::{Admitted, Permit, Request};
use fastctx_types::AccountName;
use fastctx_types::contract::Pool;
use fastctx_types::ipc::{CapacityReason, Failure};
use tokio::sync::oneshot;

pub(crate) use diagnostics::{dump, snapshot, stuck};

use crate::Node;

type Account = (HubId, AccountName);
type Subject = (Account, String);
type Settlement = (Pool, Subject);

#[derive(Clone, Copy)]
struct Bounds {
    all: usize,
    account: usize,
    conversation: usize,
}

fn bounds(pool: Pool) -> Bounds {
    static UNITS: LazyLock<usize> = LazyLock::new(|| {
        let cpu = std::thread::available_parallelism().map_or(1, |n| n.get());
        let memory = fastctx_platform::memory::physical().unwrap_or(256 << 20) / (256 << 20);
        cpu.min(memory as usize).clamp(1, 64)
    });
    // A stream mostly waits: what it holds is its buffers, bounded on their
    // own (`fastctx_net::watch_queue`). Dozens of people may watch one
    // conversation.
    static STREAMS: LazyLock<usize> = LazyLock::new(|| {
        (fastctx_platform::memory::physical().unwrap_or(1 << 30) / (256 << 10)).clamp(512, 65_536) as usize
    });
    let (all, conversation) = match pool {
        Pool::Draft => ((*UNITS * 16).max(32), 8),
        Pool::Input => ((*UNITS * 16).max(32), 8),
        Pool::Stop => ((*UNITS * 8).max(32), 8),
        Pool::Short => ((*UNITS * 32).max(64), 8),
        Pool::Change => ((*UNITS * 16).max(32), 3),
        Pool::Long => ((*UNITS * 16).max(64), 16),
        Pool::Stream => (*STREAMS, *STREAMS),
        Pool::Control => ((*UNITS * 8).max(16), 4),
        Pool::Receiving => ((*UNITS * 4).max(8), 2),
    };
    Bounds { all, account: (all / 4).max(conversation), conversation }
}

/// How long a call waits its turn: half its own time, the other half left
/// to do it, and at least two seconds; a stream, which goes on for as long
/// as it does, for as long as it has to say it is open.
fn patience(request: &Request) -> Duration {
    request.within.map_or(fastctx_types::contract::OPENED_WITHIN, |within| within / 2).max(Duration::from_secs(2))
}

/// What a waiting or running call is reckoned to hold besides its bytes.
const HELD: usize = 2 << 10;

/// The memory the calls of one account may hold here, and the calls of
/// everyone: a small share of the machine's, and a larger one.
fn budgets() -> (usize, usize) {
    static BUDGETS: LazyLock<(usize, usize)> = LazyLock::new(|| {
        let physical = fastctx_platform::memory::physical().unwrap_or(1 << 30);
        ((physical / 64).clamp(8 << 20, 256 << 20) as usize, (physical / 16).clamp(64 << 20, 2 << 30) as usize)
    });
    *BUDGETS
}

#[derive(Default)]
struct State {
    next: u64,
    calls: HashMap<u64, Entry>,
    pools: HashMap<Pool, queue::PoolQueue>,
    /// What each account's calls hold, of every kind, and everyone's.
    held: HashMap<Account, usize>,
    all_held: usize,
    running_subjects: HashMap<(Pool, Subject), usize>,
    settling: HashMap<Settlement, usize>,
}

struct Entry {
    request: Request,
    account: Account,
    subject: Subject,
    cost: usize,
    phase: Phase,
    wake: Option<oneshot::Sender<Result<(), Failure>>>,
    task: Option<tokio::task::Id>,
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Queued,
    Running,
    Settling,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn count<K: Eq + std::hash::Hash>(map: &HashMap<K, usize>, key: &K) -> usize {
    map.get(key).copied().unwrap_or(0)
}
fn decrease<K: Eq + std::hash::Hash>(map: &mut HashMap<K, usize>, key: K) {
    if let Some(n) = map.get_mut(&key) {
        *n -= 1;
        if *n == 0 {
            map.remove(&key);
        }
    }
}

fn settling_failure(me: &fastctx_types::MachineName, request: &Request, n: usize) -> Failure {
    let conversation = request.conversation.clone().unwrap_or_else(|| format!("calls from {}", request.machine));
    Failure::not_run(format!("{me} is still finishing {n} earlier changes to {conversation}; this one was not started"))
        .capacity(CapacityReason::Settling { unfinished: n })
}

/// Admits `request` once its turn comes. Cancellation drops the same guard
/// whether the call is queued or has just been admitted.
pub(crate) async fn take(node: &Node, hub: HubId, request: Request) -> Result<Admitted, Failure> {
    admit(node.machine(), hub, request, |told| node.log.line(told)).await
}

/// `take` for the machine `me`, telling `tell` who holds the places when
/// the call cannot have one.
async fn admit(
    me: fastctx_types::MachineName,
    hub: HubId,
    request: Request,
    tell: impl Fn(String),
) -> Result<Admitted, Failure> {
    let deadline = request.received + patience(&request);
    let account = (hub, request.account.clone());
    let subject =
        (account.clone(), request.conversation.clone().unwrap_or_else(|| format!("machine:{}", request.machine)));
    let (send, receive) = oneshot::channel();
    let id = {
        let mut state = state();
        let pool = request.pool;
        let cost = request.bytes.saturating_add(HELD);
        let (account_budget, budget) = budgets();
        let mine = count(&state.held, &account).saturating_add(cost) > account_budget;
        if mine || state.all_held.saturating_add(cost) > budget {
            let failure = if mine {
                let calls = state.calls.values().filter(|entry| entry.account == account).count();
                Failure::not_run(format!(
                    "{me} holds {calls} calls from {} already, as many as one account may; this one was not started",
                    request.account
                ))
            } else {
                Failure::not_run(format!(
                    "{me} holds {} calls already, as many as it can; this one was not started",
                    state.calls.len()
                ))
            };
            let told = occupancy::describe(&state, pool, &request, &failure.message);
            drop(state);
            if let Some(told) = told {
                tell(told);
            }
            return Err(failure.capacity(CapacityReason::Pending));
        }
        *state.held.entry(account.clone()).or_default() += cost;
        state.all_held += cost;
        state.next += 1;
        let id = state.next;
        state.pools.entry(pool).or_default().push(account.clone(), subject.clone(), id);
        state
            .calls
            .insert(id, Entry { request, account, subject, cost, phase: Phase::Queued, wake: Some(send), task: None });
        state.pump(pool);
        id
    };
    let permit = Arc::new(Place { id });
    match tokio::time::timeout_at(deadline, receive).await {
        Ok(Ok(result)) => result?,
        Ok(Err(_)) => return Err(Failure::not_run(format!("{me} stopped admitting this call; it was not started"))),
        Err(_) => {
            let state = state();
            let entry = &state.calls[&id];
            let pool = entry.request.pool;
            let settling = count(&state.settling, &(pool, entry.subject.clone()));
            let failure = if entry.request.finish && pool != Pool::Stop && settling >= bounds(pool).conversation {
                settling_failure(&me, &entry.request, settling)
            } else {
                let waited = patience(&entry.request).as_secs();
                Failure::not_run(format!(
                    "{me} was too busy to start {} within {waited} s; it was not started",
                    entry.request.kind
                ))
                .capacity(CapacityReason::Waiting { seconds: waited })
            };
            let told = occupancy::describe(&state, pool, &entry.request, &failure.message);
            drop(state);
            if let Some(told) = told {
                tell(told);
            }
            return Err(failure);
        }
    }
    Ok(permit)
}

struct Place {
    id: u64,
}
impl Permit for Place {
    fn started(&self) {
        if let Some(entry) = state().calls.get_mut(&self.id) {
            entry.task = tokio::task::try_id();
        }
    }
    fn settling(&self) {
        let mut state = state();
        let Some(mut entry) = state.calls.remove(&self.id) else { return };
        let pool = entry.request.pool;
        if entry.phase == Phase::Running {
            state.pools.get_mut(&pool).expect("pool").release(&entry.account);
            decrease(&mut state.running_subjects, (pool, entry.subject.clone()));
            entry.phase = Phase::Settling;
            if entry.request.finish {
                *state.settling.entry((pool, entry.subject.clone())).or_default() += 1;
            }
        }
        state.calls.insert(self.id, entry);
        state.pump(pool);
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let mut state = state();
        let Some(entry) = state.calls.remove(&self.id) else { return };
        let pool = entry.request.pool;
        if entry.phase == Phase::Running {
            state.pools.get_mut(&pool).expect("pool").release(&entry.account);
            decrease(&mut state.running_subjects, (pool, entry.subject.clone()));
        }
        if entry.phase == Phase::Queued {
            state.pools.get_mut(&pool).expect("pool").remove(&entry.account, &entry.subject, self.id);
        }
        if entry.phase == Phase::Settling && entry.request.finish {
            decrease(&mut state.settling, (pool, entry.subject.clone()));
        }
        if let Some(held) = state.held.get_mut(&entry.account) {
            *held -= entry.cost;
            if *held == 0 {
                state.held.remove(&entry.account);
            }
        }
        state.all_held -= entry.cost;
        state.pump(pool);
    }
}

impl State {
    fn pump(&mut self, pool: Pool) {
        let limits = bounds(pool);
        let Some(queue) = self.pools.get_mut(&pool) else { return };
        let calls = &mut self.calls;
        let settling = &self.settling;
        let running = &mut self.running_subjects;
        queue.pump(limits, |id| {
            let entry = calls.get_mut(&id).expect("queued call");
            let subject = (pool, entry.subject.clone());
            // A change waits while its conversation finishes as many earlier
            // ones as it may, or runs as many of its kind.
            if entry.request.finish && pool != Pool::Stop && count(settling, &subject) >= limits.conversation
                || count(running, &subject) >= limits.conversation
            {
                return queue::Decision::Wait;
            }
            *running.entry(subject).or_default() += 1;
            entry.phase = Phase::Running;
            if let Some(wake) = entry.wake.take() {
                let _ = wake.send(Ok(()));
            }
            queue::Decision::Run
        });
    }
}
