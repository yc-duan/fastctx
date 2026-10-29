//! Who holds this machine's places when a call cannot have one, for the
//! node's log: at most once a while for each kind, so that a refusal shows
//! what the places were taken by (2026-10-29: the 64 places a client's
//! words were refused for could not be told afterwards).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use fastctx_net::link::admission::Request;
use fastctx_types::contract::Pool;

use super::{Phase, State};

/// How long after telling of a kind its places are told of again.
const AGAIN: Duration = Duration::from_secs(30);

/// The most holders named.
const NAMED: usize = 8;

/// What to log of the calls of `pool` as `request` is refused for `why`;
/// nothing when that kind was told of a moment ago.
pub(super) fn describe(state: &State, pool: Pool, request: &Request, why: &str) -> Option<String> {
    static TOLD: LazyLock<Mutex<HashMap<Pool, Instant>>> = LazyLock::new(Default::default);
    {
        let mut told = TOLD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if told.get(&pool).is_some_and(|at| at.elapsed() < AGAIN) {
            return None;
        }
        told.insert(pool, Instant::now());
    }
    let mut holders: HashMap<(String, &str, &str), usize> = HashMap::new();
    let mut all = 0;
    for entry in state.calls.values().filter(|entry| entry.request.pool == pool) {
        all += 1;
        let phase = match entry.phase {
            Phase::Queued => "waiting",
            Phase::Running => "running",
            Phase::Settling => "finishing",
        };
        *holders.entry((entry.request.account.to_string(), entry.request.kind.as_str(), phase)).or_default() += 1;
    }
    let mut holders: Vec<_> = holders.into_iter().collect();
    holders.sort_by(|a, b| b.1.cmp(&a.1));
    let named: Vec<_> = holders
        .iter()
        .take(NAMED)
        .map(|((account, kind, phase), n)| format!("{n} {kind} of {account} {phase}"))
        .collect();
    Some(format!(
        "refused {} of {} from {}: {why}; {all} calls of its kind ({pool:?}) here, {} accounts' in all: {}",
        request.kind,
        request.account,
        request.machine,
        state.held.len(),
        named.join(", ")
    ))
}
