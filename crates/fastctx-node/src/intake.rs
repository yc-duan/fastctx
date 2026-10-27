//! Bounded admission, with one unit of deficit per account and conversation.
//! Calls have equal scheduling cost: hierarchical deficit round robin with a
//! quantum of one is round robin. Running work is never preempted.

mod diagnostics;
mod queue;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use fastctx_net::ids::HubId;
use fastctx_net::link::admission::{Admitted, Permit, Request};
use fastctx_types::contract::Pool;
use fastctx_types::ipc::Failure;
use fastctx_types::{AccountName, MachineName};
use tokio::sync::oneshot;

pub(crate) use diagnostics::{dump, snapshot, stuck};

type Account = (HubId, AccountName);
type Machine = (Account, MachineName);
type Subject = (Account, String);

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
        Pool::Input => ((*UNITS * 16).max(32), 3, 2),
        Pool::InputControl => ((*UNITS * 8).max(16), 1, 2),
        Pool::Short => ((*UNITS * 32).max(64), 8, 5),
        Pool::Change => ((*UNITS * 16).max(32), 3, 10),
        Pool::Long => ((*UNITS * 16).max(32), 16, 10),
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
    /// Reservations survive the processing permit. A started change can
    /// always finish without exceeding the per-conversation settlement bound.
    finishing: HashMap<String, usize>,
    settling: HashMap<String, usize>,
}

struct Entry {
    request: Request,
    account: Account,
    machine: Machine,
    subject: Subject,
    final_subject: String,
    phase: Phase,
    wake: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::Id>,
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Queued,
    Running,
    Settling,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);

fn finishing_limit(pool: Pool) -> usize {
    if pool == Pool::InputControl { 1 } else { 3 }
}

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

/// The future owns its queue entry from the first poll; cancellation removes
/// the entry even if it was admitted immediately before cancellation.
pub(crate) async fn take(me: MachineName, hub: HubId, request: Request) -> Result<Admitted, Failure> {
    let limits = bounds(request.pool);
    let deadline = request.received + limits.waiting;
    let account = (hub, request.account.clone());
    let machine = (account.clone(), request.machine.clone());
    let subject =
        (account.clone(), request.conversation.clone().unwrap_or_else(|| format!("machine:{}", request.machine)));
    let final_subject = format!("{}:{}:{}:{}", account.0, account.1, subject.1, request.pool == Pool::InputControl);
    let finishing_limit = finishing_limit(request.pool);
    let (send, receive) = oneshot::channel();
    let id = {
        let mut state = state();
        if request.finish && count(&state.settling, &final_subject) >= finishing_limit {
            return Err(Failure::not_run(format!(
                "{} still has {finishing_limit} unfinished changes; no new change was started",
                subject.1
            )));
        }
        let pool = request.pool;
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
                "{me} has reached the pending {pool:?} call limit for {} from {}; no work was started",
                request.account, request.machine
            )));
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
                request,
                account,
                machine,
                subject,
                final_subject,
                phase: Phase::Queued,
                wake: Some(send),
                task: None,
            },
        );
        state.pump(pool);
        id
    };
    let permit = Arc::new(Place { id });
    if tokio::time::timeout_at(deadline, receive).await.is_err() {
        let state = state();
        let entry = &state.calls[&id];
        return Err(Failure::not_run(format!(
            "{me} could not start {} for {} within {} s: waiting for {:?} capacity; no work was started",
            entry.request.kind,
            entry.request.account,
            limits.waiting.as_secs(),
            entry.request.pool
        )));
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
        if entry.phase == Phase::Running {
            state.pools.get_mut(&entry.request.pool).expect("pool").release(&entry.account);
            decrease(&mut state.running_subjects, (entry.request.pool, entry.subject.clone()));
            entry.phase = Phase::Settling;
            if entry.request.finish {
                *state.settling.entry(entry.final_subject.clone()).or_default() += 1;
            }
        }
        let pool = entry.request.pool;
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
            decrease(&mut state.settling, entry.final_subject.clone());
        }
        if entry.phase != Phase::Queued && entry.request.finish {
            decrease(&mut state.finishing, entry.final_subject.clone());
        }
        if let Some(bytes) = state.bytes.get_mut(&pool) {
            *bytes -= entry.request.bytes;
        }
        decrease(&mut state.totals, pool);
        decrease(&mut state.accounts, (pool, entry.account));
        decrease(&mut state.machines, (pool, entry.machine));
        decrease(&mut state.subjects, (pool, entry.subject));
        state.pump(pool);
        // A reservation is shared across pools: releasing one can unblock input.
        for other in
            [Pool::InputControl, Pool::Input, Pool::Change, Pool::Long, Pool::Stream, Pool::Control, Pool::Short]
        {
            if other != pool {
                state.pump(other);
            }
        }
    }
}

impl State {
    fn pump(&mut self, pool: Pool) {
        let limits = bounds(pool);
        let Some(queue) = self.pools.get_mut(&pool) else { return };
        let calls = &mut self.calls;
        let finishing = &mut self.finishing;
        let running = &mut self.running_subjects;
        queue.pump(limits, |id| {
            let entry = calls.get_mut(&id).expect("queued call");
            let subject_key = (pool, entry.subject.clone());
            if count(running, &subject_key) >= limits.conversation {
                return false;
            }
            if entry.request.finish && count(finishing, &entry.final_subject) >= finishing_limit(pool) {
                return false;
            }
            *running.entry(subject_key).or_default() += 1;
            if entry.request.finish {
                *finishing.entry(entry.final_subject.clone()).or_default() += 1;
            }
            entry.phase = Phase::Running;
            if let Some(wake) = entry.wake.take() {
                let _ = wake.send(());
            }
            true
        });
    }
}
