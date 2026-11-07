//! Read admission rotates accounts, then connections, before any result is built.

mod work;
pub(crate) use work::{blocking, run};

use crate::outq::Outq;
use fastctx_types::AccountName;
use std::{
    collections::BTreeMap,
    ops::Bound::{Excluded, Unbounded},
    sync::{Arc, LazyLock, Mutex},
};
use tokio::{sync::oneshot, time::Instant};

pub(crate) static DOCUMENTS: LazyLock<Arc<Readers>> = LazyLock::new(|| Readers::new(8));
pub(crate) static PAGES: LazyLock<Arc<Readers>> = LazyLock::new(|| Readers::new(32));
pub(crate) const WITHIN: std::time::Duration = std::time::Duration::from_secs(55);

pub(crate) async fn account(
    hub: &crate::Hub,
    from: &crate::origin::Origin,
    account: &AccountName,
    out: &Outq,
    deadline: Instant,
) -> Result<Permit, String> {
    {
        let store = hub.store();
        from.check(&store.book.directory)?;
        store
            .indexed()
            .authorize(account, fastctx_net::authorization::Action::ActFor(from))
            .map_err(|error| error.to_string())?;
    }
    DOCUMENTS.enter(Some(account), out, deadline).await
}

pub(crate) fn owned(request: &fastctx_types::owned_records::Request) -> bool {
    use fastctx_types::{account_records, owned_records::Request};
    !matches!(
        request,
        Request::Register { .. }
            | Request::Delete { .. }
            | Request::PrepareDeletion { .. }
            | Request::Record {
                request: account_records::Request::Write { .. } | account_records::Request::Delete { .. },
                ..
            }
    )
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
enum Account {
    Named(AccountName),
    Connection(usize),
}

type Waiting = BTreeMap<u64, oneshot::Sender<Permit>>;

#[derive(Default)]
struct Connections {
    queues: BTreeMap<usize, Waiting>,
    last: Option<usize>,
    pending: usize,
}

#[derive(Default)]
struct State {
    accounts: BTreeMap<Account, Connections>,
    last: Option<Account>,
    next: u64,
    active: usize,
    pending: usize,
}

pub(crate) struct Readers {
    limit: usize,
    state: Mutex<State>,
}

pub(crate) struct Permit {
    readers: Arc<Readers>,
    active: bool,
}

struct Queued {
    readers: Arc<Readers>,
    account: Account,
    connection: usize,
    id: u64,
}

impl Readers {
    fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self { limit, state: Mutex::default() })
    }

    pub async fn enter(
        self: &Arc<Self>,
        account: Option<&AccountName>,
        out: &Outq,
        deadline: Instant,
    ) -> Result<Permit, String> {
        let connection = out.identity();
        let account = account.map_or(Account::Connection(connection), |name| Account::Named(name.clone()));
        // Bound pathological queues by host memory (2026-11-07), separately
        // from disk concurrency, so a stalled connection cannot exhaust the hub.
        static WAITING: LazyLock<usize> = LazyLock::new(|| {
            (fastctx_platform::memory::physical().unwrap_or(1 << 30) / (64 << 10)).clamp(4096, 262_144) as usize
        });
        let budget = *WAITING;
        let (send, receive) = oneshot::channel();
        let queued = {
            let mut state = self.state.lock().expect("never poisoned");
            if state.pending >= budget
                || state.accounts.get(&account).is_some_and(|connections| {
                    connections.pending >= budget / 4
                        || connections.queues.get(&connection).is_some_and(|queue| queue.len() >= budget / 16)
                })
            {
                return Err(
                    "too many read requests are waiting for this account or connection; retry after they finish".into(),
                );
            }
            state.pending += 1;
            state.accounts.entry(account.clone()).or_default().pending += 1;
            let id = state.next;
            state.next = state.next.checked_add(1).expect("reader identity exhausted");
            state.accounts.entry(account.clone()).or_default().queues.entry(connection).or_default().insert(id, send);
            self.dispatch(&mut state);
            Queued { readers: self.clone(), account, connection, id }
        };
        let result = tokio::select! {
            biased;
            () = out.closed() => Err("the reader's connection closed".into()),
            result = tokio::time::timeout_at(deadline, receive) => match result {
                Ok(Ok(permit)) => Ok(permit),
                _ => Err("the hub was too busy reading to admit this request within 55 seconds".into()),
            },
        };
        drop(queued);
        result
    }

    fn dispatch(self: &Arc<Self>, state: &mut State) {
        while state.active < self.limit {
            let Some(account) = next(&state.accounts, state.last.as_ref()).cloned() else { break };
            let connections = state.accounts.get_mut(&account).expect("queued account");
            let connection = *next(&connections.queues, connections.last.as_ref()).expect("queued connection");
            let waiting = connections.queues.get_mut(&connection).expect("queued connection");
            let (_, send) = waiting.pop_first().expect("queued request");
            state.pending -= 1;
            connections.pending -= 1;
            if waiting.is_empty() {
                connections.queues.remove(&connection);
            }
            connections.last = Some(connection);
            if connections.queues.is_empty() {
                state.accounts.remove(&account);
            }
            state.last = Some(account);
            match send.send(Permit { readers: self.clone(), active: true }) {
                Ok(()) => state.active += 1,
                Err(mut permit) => permit.active = false,
            }
        }
    }
}

fn next<'a, K: Ord, V>(entries: &'a BTreeMap<K, V>, last: Option<&K>) -> Option<&'a K> {
    last.and_then(|last| entries.range((Excluded(last), Unbounded)).next())
        .or_else(|| entries.first_key_value())
        .map(|(key, _)| key)
}

impl Drop for Permit {
    fn drop(&mut self) {
        if self.active {
            let mut state = self.readers.state.lock().expect("never poisoned");
            state.active -= 1;
            self.readers.dispatch(&mut state);
        }
    }
}

impl Drop for Queued {
    fn drop(&mut self) {
        let mut state = self.readers.state.lock().expect("never poisoned");
        let mut removed = false;
        if let Some(connections) = state.accounts.get_mut(&self.account) {
            if let Some(waiting) = connections.queues.get_mut(&self.connection) {
                if waiting.remove(&self.id).is_some() {
                    connections.pending -= 1;
                    removed = true;
                }
                if waiting.is_empty() {
                    connections.queues.remove(&self.connection);
                }
            }
            if connections.queues.is_empty() {
                state.accounts.remove(&self.account);
            }
        }
        state.pending -= usize::from(removed);
    }
}
