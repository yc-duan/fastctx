//! How many calls from other machines this node answers at once
//! (`specs/world-concurrency.md`, 机器之间). Each kind of call has places of
//! its own (`fastctx_types::contract::Pool`), bounded for each machine, for
//! each account and for the node as a whole: a machine gone wrong cannot
//! bury this one, and neither long work nor many streams keeps out a
//! question or a stop. A call takes its place before anything waits on it,
//! and lets it go once it is answered.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{LazyLock, Mutex};

use fastctx_net::ids::HubId;

use fastctx_types::contract::Pool;
use fastctx_types::ipc::Failure;
use fastctx_types::{AccountName, MachineName};

/// How many calls of one kind are answered at once.
struct Bounds {
    /// From one machine.
    machine: usize,
    /// For one account, from all its machines.
    account: usize,
    /// In all.
    all: usize,
}

fn bounds(pool: Pool) -> Bounds {
    match pool {
        Pool::Short => Bounds { machine: 64, account: 128, all: 512 },
        Pool::Long => Bounds { machine: 64, account: 128, all: 256 },
        Pool::Stream => Bounds { machine: 128, account: 256, all: 1024 },
        Pool::Control => Bounds { machine: 32, account: 64, all: 256 },
        Pool::Receiving => Bounds { machine: 16, account: 32, all: 64 },
    }
}

/// What the calls of a kind are called, saying there is no place for one.
fn called(pool: Pool) -> &'static str {
    match pool {
        Pool::Short => "calls",
        Pool::Long => "long-running calls",
        Pool::Stream => "streams",
        Pool::Control => "control calls",
        Pool::Receiving => "large calls coming in",
    }
}

#[derive(Default)]
struct Taken {
    machines: HashMap<(Pool, HubId, MachineName), usize>,
    accounts: HashMap<(Pool, HubId, AccountName), usize>,
    all: HashMap<Pool, usize>,
}

static TAKEN: LazyLock<Mutex<Taken>> = LazyLock::new(Default::default);

/// The place one call takes while it is answered.
pub(crate) struct Place {
    pool: Pool,
    hub: HubId,
    machine: MachineName,
    account: AccountName,
}

/// Takes a place of the kind `pool` on `me` for a call that came from
/// `from` through the hub `hub`, or says why there is none.
pub(crate) fn take(
    me: &MachineName,
    hub: &HubId,
    machine: &MachineName,
    account: &AccountName,
    pool: Pool,
) -> Result<Place, Failure> {
    let bounds = bounds(pool);
    let by_machine = (pool, hub.clone(), machine.clone());
    let by_account = (pool, hub.clone(), account.clone());
    let mut taken = TAKEN.lock().expect("never poisoned");
    let full = |what: String| Failure::not_run(format!("{me} is answering {what} already; ask again in a moment"));
    if count(&taken.machines, &by_machine) >= bounds.machine {
        return Err(full(format!("{} {} from {}", bounds.machine, called(pool), machine)));
    }
    if count(&taken.accounts, &by_account) >= bounds.account {
        return Err(full(format!("{} {} for {}", bounds.account, called(pool), account)));
    }
    if count(&taken.all, &pool) >= bounds.all {
        return Err(full(format!("{} {}", bounds.all, called(pool))));
    }
    *taken.machines.entry(by_machine).or_insert(0) += 1;
    *taken.accounts.entry(by_account).or_insert(0) += 1;
    *taken.all.entry(pool).or_insert(0) += 1;
    Ok(Place { pool, hub: hub.clone(), machine: machine.clone(), account: account.clone() })
}

fn count<K: Eq + Hash>(counts: &HashMap<K, usize>, key: &K) -> usize {
    counts.get(key).copied().unwrap_or(0)
}

/// Lets one of the places counted under `key` go.
fn let_go<K: Eq + Hash>(counts: &mut HashMap<K, usize>, key: K) {
    if let Some(count) = counts.get_mut(&key) {
        *count -= 1;
        if *count == 0 {
            counts.remove(&key);
        }
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let mut taken = TAKEN.lock().expect("never poisoned");
        let_go(&mut taken.machines, (self.pool, self.hub.clone(), self.machine.clone()));
        let_go(&mut taken.accounts, (self.pool, self.hub.clone(), self.account.clone()));
        let_go(&mut taken.all, self.pool);
    }
}
