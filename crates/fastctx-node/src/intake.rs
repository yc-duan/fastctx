//! Bounded admission with equal-cost, two-level deficit round robin.
//! Execution limits and settlement counts are independent for each kind.

mod diagnostics;
mod queue;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use fastctx_net::ids::HubId;
use fastctx_net::link::admission::{Admitted, Permit, Request};
use fastctx_types::contract::Pool;
use fastctx_types::ipc::{CapacityReason, Failure};
use fastctx_types::{AccountName, MachineName};
use tokio::sync::oneshot;

pub(crate) use diagnostics::{dump, snapshot, stuck};

type Account = (HubId, AccountName);
type Machine = (Account, MachineName);
type Subject = (Account, String);
type Settlement = (Pool, Subject);

#[derive(Clone, Copy)]
struct Bounds {
    all: usize,
    account: usize,
    conversation: usize,
    waiting: Duration,
}

fn bounds(pool: Pool) -> Bounds {
    static UNITS: LazyLock<usize> = LazyLock::new(|| {
        let cpu = std::thread::available_parallelism().map_or(1, |n| n.get());
        let memory = fastctx_platform::memory::physical().unwrap_or(256 << 20) / (256 << 20);
        cpu.min(memory as usize).clamp(1, 64)
    });
    let (all, conversation, seconds) = match pool {
        Pool::Draft => ((*UNITS * 16).max(32), 8, 2),
        Pool::Input => ((*UNITS * 16).max(32), 8, 2),
        Pool::Stop => ((*UNITS * 8).max(32), 8, 2),
        Pool::Short => ((*UNITS * 32).max(64), 8, 5),
        Pool::Change => ((*UNITS * 16).max(32), 3, 10),
        Pool::Long => ((*UNITS * 16).max(64), 16, 10),
        Pool::Stream => ((*UNITS * 128).max(512), 32, 10),
        Pool::Control => ((*UNITS * 8).max(16), 4, 2),
        Pool::Receiving => ((*UNITS * 4).max(8), 2, 5),
    };
    Bounds { all, account: (all / 4).max(conversation), conversation, waiting: Duration::from_secs(seconds) }
}

#[derive(Default)]
struct State {
    next: u64,
    calls: HashMap<u64, Entry>,
    pools: HashMap<Pool, queue::PoolQueue>,
    accounts: HashMap<(Pool, Account), usize>,
    totals: HashMap<Pool, usize>,
    bytes: HashMap<Pool, usize>,
    machines: HashMap<(Pool, Machine), usize>,
    subjects: HashMap<(Pool, Subject), usize>,
    running_subjects: HashMap<(Pool, Subject), usize>,
    settling: HashMap<Settlement, usize>,
}

struct Entry {
    me: MachineName,
    request: Request,
    account: Account,
    machine: Machine,
    subject: Subject,
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

fn settling_failure(me: &MachineName, request: &Request, n: usize) -> Failure {
    let conversation = request.conversation.clone().unwrap_or_else(|| format!("calls from {}", request.machine));
    Failure::not_run(format!("{me} is still finishing {n} earlier changes to {conversation}; this one was not started"))
        .capacity(CapacityReason::Settling { unfinished: n })
}

/// Cancellation drops the same guard whether the call is queued or has just
/// been admitted. Only work already settling can refuse another change.
pub(crate) async fn take(me: MachineName, hub: HubId, request: Request) -> Result<Admitted, Failure> {
    let limits = bounds(request.pool);
    let deadline = request.received + limits.waiting;
    let account = (hub, request.account.clone());
    let machine = (account.clone(), request.machine.clone());
    let subject =
        (account.clone(), request.conversation.clone().unwrap_or_else(|| format!("machine:{}", request.machine)));
    let (send, receive) = oneshot::channel();
    let id = {
        let mut state = state();
        let pool = request.pool;
        let settling = count(&state.settling, &(pool, subject.clone()));
        if request.finish && pool != Pool::Stop && settling >= limits.conversation {
            return Err(settling_failure(&me, &request, settling));
        }
        static BYTE_LIMIT: LazyLock<usize> = LazyLock::new(|| {
            (fastctx_platform::memory::physical().unwrap_or(256 << 20) / 32).clamp(4 << 20, 64 << 20) as usize
        });
        let full = count(&state.bytes, &pool).saturating_add(request.bytes) > *BYTE_LIMIT
            || count(&state.totals, &pool) >= (limits.all * 8).clamp(2048, 16384)
            || count(&state.accounts, &(pool, account.clone())) >= 256
            || count(&state.machines, &(pool, machine.clone())) >= 128
            || count(&state.subjects, &(pool, subject.clone())) >= 64;
        if full {
            return Err(Failure::not_run(format!(
                "{me} has too many calls waiting from {} on {}; this one was not started",
                request.account, request.machine
            ))
            .capacity(CapacityReason::Pending));
        }
        *state.bytes.entry(pool).or_default() += request.bytes;
        *state.totals.entry(pool).or_default() += 1;
        state.next += 1;
        let id = state.next;
        *state.accounts.entry((pool, account.clone())).or_default() += 1;
        *state.machines.entry((pool, machine.clone())).or_default() += 1;
        *state.subjects.entry((pool, subject.clone())).or_default() += 1;
        state.pools.entry(pool).or_default().push(account.clone(), subject.clone(), id);
        state.calls.insert(
            id,
            Entry {
                me: me.clone(),
                request,
                account,
                machine,
                subject,
                phase: Phase::Queued,
                wake: Some(send),
                task: None,
            },
        );
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
            return Err(Failure::not_run(format!(
                "{me} was too busy to start {} within {} s; it was not started",
                entry.request.kind,
                limits.waiting.as_secs()
            ))
            .capacity(CapacityReason::Waiting { seconds: limits.waiting.as_secs() }));
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
        if let Some(bytes) = state.bytes.get_mut(&pool) {
            *bytes -= entry.request.bytes;
        }
        decrease(&mut state.totals, pool);
        decrease(&mut state.accounts, (pool, entry.account));
        decrease(&mut state.machines, (pool, entry.machine));
        decrease(&mut state.subjects, (pool, entry.subject));
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
            let n = count(settling, &subject);
            if entry.request.finish && pool != Pool::Stop && n >= limits.conversation {
                if let Some(wake) = entry.wake.take() {
                    let _ = wake.send(Err(settling_failure(&entry.me, &entry.request, n)));
                }
                return queue::Decision::Refuse;
            }
            if count(running, &subject) >= limits.conversation {
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
