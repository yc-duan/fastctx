//! A link shares one observation connection per destination and account. Peers
//! negotiate on the first Watch itself, so an old peer still serves that first
//! subscription normally and later subscriptions use its existing one-watch calls.

mod receiving;

use super::{Inner, Link};
use crate::{watch_mux, watch_queue};
use fastctx_types::{
    AccountName, MachineName,
    ipc::Failure,
    session::Watched,
    watching::{Command, Subscription},
};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{Notify, mpsc, oneshot};

type Key = (AccountName, crate::sign::PublicKey);
pub(super) type Pools = Mutex<HashMap<Key, Arc<Pool>>>;
type Pieces = watch_queue::Receiver<Result<(u64, Watched), Failure>>;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Opening,
    Multiplexed,
    Legacy,
    Closed,
}
enum Opening {
    Separate,
    Restart,
    Failed(Failure),
}
struct Slot {
    pieces: watch_queue::Sender<Result<(u64, Watched), Failure>>,
    opened: Option<oneshot::Sender<Result<(), Opening>>>,
    pending: Option<Subscription>,
    memory: Option<Arc<crate::reassembly::Reservation>>,
    next: u64,
}
struct State {
    mode: Mode,
    next: u64,
    slots: BTreeMap<u64, Slot>,
}
struct Queued {
    command: Command,
    _memory: Arc<crate::reassembly::Reservation>,
}
pub(super) struct Pool {
    state: Mutex<State>,
    commands: mpsc::Sender<Queued>,
    stopped: Notify,
    acknowledgements: Mutex<BTreeMap<u64, u64>>,
    acknowledged: Notify,
    watch: u64,
    key: Key,
    name: MachineName,
    owner: Weak<Inner>,
    registered: bool,
}

/// One subscription. Dropping it unsubscribes only it, even while other
/// conversations and their composers use the same connection.
pub struct Watching {
    pool: Arc<Pool>,
    id: u64,
    pieces: Pieces,
}
#[derive(Clone)]
pub struct Input {
    pool: Arc<Pool>,
    id: u64,
}
impl Input {
    /// Enqueues one batch on this authenticated stream. The receiver's Saved
    /// packet, rather than this enqueue, acknowledges its durable effect.
    pub fn send(&self, input: fastctx_types::composer::sync::Input) -> Result<(), Failure> {
        {
            let state = self.pool.state.lock().expect("never poisoned");
            if state.mode != Mode::Multiplexed || !state.slots.contains_key(&self.id) {
                return Err(Failure::not_run("the shared document connection is unavailable"));
            }
        }
        self.pool.command(Command::Composer { id: self.id, input: Box::new(input) })
    }
}
impl Watching {
    pub fn input(&self) -> Option<Input> {
        (self.pool.mode() == Mode::Multiplexed).then(|| Input { pool: self.pool.clone(), id: self.id })
    }
    pub async fn next(&mut self) -> Result<Option<Watched>, Failure> {
        match self.pieces.recv().await.map_err(|e| Failure::new(e.to_string()))? {
            Some(Ok((number, watched))) => {
                if self.pool.mode() == Mode::Multiplexed {
                    self.pool.command(Command::Consumed { id: self.id, number })?;
                }
                Ok(Some(watched))
            }
            Some(Err(failure)) => Err(failure),
            None => Ok(None),
        }
    }
}
impl Drop for Watching {
    fn drop(&mut self) {
        self.pool.remove(self.id);
    }
}

impl Link {
    pub async fn watch_shared(
        self: &Arc<Self>,
        machine: &MachineName,
        account: &AccountName,
        request: Subscription,
    ) -> Result<Watching, Failure> {
        let identity = self
            .inner
            .key_of(machine)
            .await
            .ok_or_else(|| Failure::not_run(format!("there is no machine named {machine}")))?;
        let key = (account.clone(), identity);
        let name = self.view().member(machine).map_or_else(|| machine.clone(), |m| m.name.clone());
        let mut separate = false;
        loop {
            let mut start = None;
            let (watching, opened) = {
                let mut pools = self.inner.watching.lock().expect("never poisoned");
                if let Some(pool) = pools.get(&key).cloned().filter(|_| !separate) {
                    drop(pools);
                    match pool.register(request.clone()) {
                        Ok(pair) => pair,
                        Err(Opening::Separate) => {
                            separate = true;
                            continue;
                        }
                        Err(Opening::Restart) => {
                            pool.finish(None);
                            continue;
                        }
                        Err(Opening::Failed(failure)) => return Err(failure),
                    }
                } else {
                    let (pool, commands) = Pool::new(&self.inner, key.clone(), name.clone(), !separate)?;
                    let pair = pool.register(request.clone()).map_err(|e| match e {
                        Opening::Failed(failure) => failure,
                        Opening::Separate | Opening::Restart => Failure::new("a fresh watch did not open"),
                    })?;
                    if !separate {
                        pools.insert(key.clone(), pool.clone());
                    }
                    start = Some((pool, commands));
                    pair
                }
            };
            if let Some((pool, commands)) = start {
                pool.start(self.clone(), request.clone(), commands, !separate);
            }
            match opened.await {
                Ok(Ok(())) => return Ok(watching),
                Ok(Err(Opening::Separate)) => {
                    drop(watching);
                    separate = true;
                }
                Ok(Err(Opening::Restart)) => {
                    drop(watching);
                    separate = false;
                }
                Ok(Err(Opening::Failed(failure))) => return Err(failure),
                Err(_) => return Err(Failure::new("the watch closed before its subscription opened")),
            }
        }
    }
}

impl Pool {
    fn new(
        owner: &Arc<Inner>,
        key: Key,
        name: MachineName,
        registered: bool,
    ) -> Result<(Arc<Self>, mpsc::Receiver<Queued>), Failure> {
        let (commands, receive) = mpsc::channel(watch_mux::SUBSCRIPTIONS * 2 + 1);
        Ok((
            Arc::new(Self {
                state: Mutex::new(State { mode: Mode::Opening, next: 0, slots: BTreeMap::new() }),
                commands,
                stopped: Notify::new(),
                acknowledgements: Mutex::new(BTreeMap::new()),
                acknowledged: Notify::new(),
                watch: u64::from_le_bytes(crate::sign::random::<8>().map_err(|e| Failure::new(e.to_string()))?),
                key,
                name,
                owner: Arc::downgrade(owner),
                registered,
            }),
            receive,
        ))
    }
    fn mode(&self) -> Mode {
        self.state.lock().expect("never poisoned").mode
    }

    fn register(
        self: &Arc<Self>,
        request: Subscription,
    ) -> Result<(Watching, oneshot::Receiver<Result<(), Opening>>), Opening> {
        let size = crate::watch_queue::size(&request);
        let memory = Arc::new(
            crate::reassembly::Reservation::new(size.saturating_mul(2).saturating_add(1024))
                .map_err(|e| Opening::Failed(Failure::not_run(e)))?,
        );
        let mut state = self.state.lock().expect("never poisoned");
        if state.mode == Mode::Legacy {
            return Err(Opening::Separate);
        }
        if state.mode == Mode::Closed {
            return Err(Opening::Restart);
        }
        if state.slots.len() >= watch_mux::SUBSCRIPTIONS {
            return Err(Opening::Failed(Failure::not_run("the shared watch has reached its subscription limit")));
        }
        let id = state
            .next
            .checked_add(1)
            .ok_or_else(|| Opening::Failed(Failure::new("watch subscription numbers exhausted")))?;
        state.next = id;
        let (sender, pieces) = watch_queue::channel(|piece: &Result<(u64, Watched), Failure>| match piece {
            Ok((_, watched)) => watch_queue::watched_size(watched),
            Err(error) => (error.message.len(), None),
        });
        let (opened, ready) = oneshot::channel();
        let pending = if state.mode == Mode::Multiplexed {
            self.commands
                .try_send(Queued {
                    command: Command::Subscribe { id, subscription: Box::new(request) },
                    _memory: memory.clone(),
                })
                .map_err(|_| Opening::Failed(Failure::not_run("the shared watch command queue is full")))?;
            None
        } else {
            Some(request)
        };
        state.slots.insert(id, Slot { pieces: sender, opened: Some(opened), pending, memory: Some(memory), next: 0 });
        Ok((Watching { pool: self.clone(), id, pieces }, ready))
    }

    fn start(
        self: Arc<Self>,
        link: Arc<Link>,
        initial: Subscription,
        commands: mpsc::Receiver<Queued>,
        multiplex: bool,
    ) {
        let pool = self.clone();
        let feed_link = link.clone();
        let feed: super::Feed = Box::new(move |stream| Box::pin(receiving::run(pool, feed_link, commands, stream)));
        tokio::spawn(async move {
            let stopping = self.stopped.notified();
            tokio::pin!(stopping);
            stopping.as_mut().enable();
            if self.mode() == Mode::Closed {
                return;
            }
            let result = tokio::select! {
                () = &mut stopping => return,
                result = link.call_with_stream(&self.name, &self.key.0, fastctx_types::access::Purpose::Agents, initial.call(self.watch, multiplex), feed) => result,
            };
            let failure = match result {
                Ok(Ok(fastctx_types::remote::RemoteReply::Watched)) => None,
                Ok(Ok(_)) => Some(Failure::new("the machine answered something other than a watch")),
                Ok(Err(failure)) => Some(failure),
                Err(error) => Some(Failure::new(error.to_string())),
            };
            self.finish(failure);
        });
    }

    fn command(&self, command: Command) -> Result<(), Failure> {
        if let Command::Consumed { id, number } = command {
            let state = self.state.lock().expect("never poisoned");
            if state.slots.contains_key(&id) {
                self.acknowledgements
                    .lock()
                    .expect("never poisoned")
                    .entry(id)
                    .and_modify(|previous| *previous = (*previous).max(number))
                    .or_insert(number);
                self.acknowledged.notify_one();
            }
            return Ok(());
        }
        let memory = Arc::new(
            crate::reassembly::Reservation::new(crate::watch_queue::size(&command) + 256).map_err(Failure::new)?,
        );
        if self.commands.try_send(Queued { command, _memory: memory }).is_err() {
            let failure = Failure::new("the shared watch command queue is full or closed");
            self.finish(Some(failure.clone()));
            return Err(failure);
        }
        Ok(())
    }

    fn remove(&self, id: u64) {
        let (mode, empty, removed) = {
            let mut state = self.state.lock().expect("never poisoned");
            let removed = state.slots.remove(&id).is_some();
            let mode = state.mode;
            let empty = state.slots.is_empty();
            if empty {
                state.mode = Mode::Closed;
            }
            (mode, empty, removed)
        };
        self.acknowledgements.lock().expect("never poisoned").remove(&id);
        if empty {
            self.finish(None);
        } else if removed && mode == Mode::Multiplexed {
            let _ = self.command(Command::Unsubscribe { id });
        }
    }

    fn finish(&self, failure: Option<Failure>) {
        let (mode, slots) = {
            let mut state = self.state.lock().expect("never poisoned");
            let mode = state.mode;
            state.mode = Mode::Closed;
            (mode, std::mem::take(&mut state.slots))
        };
        for (id, mut slot) in slots {
            if let Some(opened) = slot.opened.take() {
                let why = if id != 1
                    && mode == Mode::Opening
                    && failure
                        .as_ref()
                        .is_some_and(|failure| failure.not_run || failure.is_deleted() || failure.is_refused())
                {
                    Opening::Restart
                } else {
                    Opening::Failed(
                        failure
                            .clone()
                            .unwrap_or_else(|| Failure::new("the watch ended before its subscription opened")),
                    )
                };
                let _ = opened.send(Err(why));
            }
            if let Some(failure) = &failure {
                let _ = slot.pieces.send(Err(failure.clone()));
            }
        }
        self.acknowledgements.lock().expect("never poisoned").clear();
        if self.registered
            && let Some(owner) = self.owner.upgrade()
        {
            let mut pools = owner.watching.lock().expect("never poisoned");
            if pools.get(&self.key).is_some_and(|pool| std::ptr::eq(Arc::as_ptr(pool), self)) {
                pools.remove(&self.key);
            }
        }
        self.stopped.notify_waiters();
    }
}
