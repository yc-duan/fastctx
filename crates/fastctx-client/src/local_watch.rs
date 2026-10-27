//! One local observation connection per World, negotiated with the node. Dropping
//! a viewer unsubscribes that viewer; the last one closes the shared connection.

mod connection;

use crate::{CallError, Error, World};
use fastctx_net::{
    watch_ipc::{self, PendingCommand},
    watch_queue,
};
use fastctx_types::{
    Call,
    ipc::{ClientFrame, Failure},
    session::Watched,
    watching::local::{Command, Subscription},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::{Notify, mpsc};

type Piece = Result<(u64, Watched), Failure>;

#[derive(Default)]
pub(crate) struct Pool {
    shared: tokio::sync::Mutex<Weak<Shared>>,
}
impl Pool {
    pub async fn watch(&self, world: &World, request: Subscription) -> Result<crate::Watching, Error> {
        tokio::time::timeout(watch_ipc::TIMEOUT, self.open(world, request))
            .await
            .map_err(|_| Error::Other("opening the local shared watch did not finish within 30 s".into()))?
    }

    async fn open(&self, world: &World, request: Subscription) -> Result<crate::Watching, Error> {
        let mut slot = self.shared.lock().await;
        if let Some(shared) = slot.upgrade()
            && !shared.closed()
        {
            return shared.subscribe(request).map(watching).map_err(Error::Call);
        }
        let mut client = world.connect().await?;
        if !client.watch_many {
            return Ok(client.watch(request.call()).await?);
        }
        if request.goal.is_some() && !client.goal_runtime {
            return Err(Error::Call(older_goal()));
        }
        client.next += 1;
        client
            .connection
            .send(&crate::to_json(&ClientFrame::Call { id: client.next, call: Call::WatchMany }))
            .await
            .map_err(CallError::Broken)?;
        let (commands, incoming) = mpsc::channel(2 * watch_ipc::SUBSCRIPTIONS + 1);
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            commands,
            wake: Notify::new(),
            goal_runtime: client.goal_runtime,
        });
        let watching = shared.subscribe(request)?;
        *slot = Arc::downgrade(&shared);
        tokio::spawn(connection::run(shared, client.connection, incoming));
        Ok(self::watching(watching))
    }
}

fn watching(watching: Watching) -> crate::Watching {
    crate::Watching { transport: crate::WatchingTransport::LocalMany(watching) }
}
fn older_goal() -> CallError {
    CallError::Failed(Failure::not_run(fastctx_types::goals::words::OLDER_NODE))
}

struct Slot {
    sender: watch_queue::Sender<Piece>,
    expected: u64,
}
#[derive(Default)]
struct State {
    next: u64,
    slots: HashMap<u64, Slot>,
    acknowledged: HashMap<u64, u64>,
    closed: bool,
}
struct Shared {
    state: Mutex<State>,
    commands: mpsc::Sender<PendingCommand>,
    wake: Notify,
    goal_runtime: bool,
}
impl Shared {
    fn closed(&self) -> bool {
        self.state.lock().expect("local watch state").closed
    }

    fn subscribe(self: &Arc<Self>, request: Subscription) -> Result<Watching, CallError> {
        if request.goal.is_some() && !self.goal_runtime {
            return Err(older_goal());
        }
        let mut state = self.state.lock().expect("local watch state");
        if state.closed {
            return Err(CallError::Failed(Failure::new("the local shared watch closed")));
        }
        if state.slots.len() >= watch_ipc::SUBSCRIPTIONS {
            return Err(CallError::Failed(Failure::not_run(
                "the local shared watch has reached its subscription limit",
            )));
        }
        state.next = state
            .next
            .checked_add(1)
            .ok_or_else(|| CallError::Failed(Failure::new("the local watch exhausted its subscription numbers")))?;
        let id = state.next;
        let command = PendingCommand::new(&Command::Subscribe { id, subscription: Box::new(request) })
            .map_err(CallError::Broken)?;
        self.commands
            .try_send(command)
            .map_err(|_| CallError::Failed(Failure::not_run("the local watch command queue is full")))?;
        let (sender, receiver) = watch_queue::channel(|piece: &Piece| match piece {
            Ok((_, piece)) => watch_queue::watched_size(piece),
            Err(error) => (error.message.len(), None),
        });
        state.slots.insert(id, Slot { sender, expected: 0 });
        Ok(Watching { shared: self.clone(), id, receiver })
    }

    fn remove(&self, id: u64) {
        let mut state = self.state.lock().expect("local watch state");
        if state.slots.remove(&id).is_some() {
            state.acknowledged.remove(&id);
            if state.slots.is_empty() {
                state.closed = true;
            } else if PendingCommand::new(&Command::Unsubscribe { id })
                .ok()
                .is_none_or(|command| self.commands.try_send(command).is_err())
            {
                Self::fail(&mut state, Failure::new("the local watch could not send its unsubscribe"));
            }
        }
        self.wake.notify_one();
    }

    fn consumed(&self, id: u64, number: u64) {
        let mut state = self.state.lock().expect("local watch state");
        if state.slots.contains_key(&id) {
            state.acknowledged.entry(id).and_modify(|through| *through = (*through).max(number)).or_insert(number);
            self.wake.notify_one();
        }
    }

    fn fail(state: &mut State, failure: Failure) {
        state.closed = true;
        state.acknowledged.clear();
        for (_, slot) in state.slots.drain() {
            let _ = slot.sender.send(Err(failure.clone()));
        }
    }
}

pub(crate) struct Watching {
    shared: Arc<Shared>,
    id: u64,
    receiver: watch_queue::Receiver<Piece>,
}
impl Watching {
    pub async fn next(&mut self) -> Result<Option<Watched>, CallError> {
        match self.receiver.recv().await {
            Ok(Some(Ok((number, piece)))) => {
                self.shared.consumed(self.id, number);
                Ok(Some(piece))
            }
            Ok(Some(Err(failure))) => {
                self.shared.remove(self.id);
                Err(CallError::Failed(failure))
            }
            Ok(None) => Ok(None),
            Err(error) => {
                self.shared.remove(self.id);
                Err(CallError::Failed(Failure::new(error.to_string())))
            }
        }
    }
}
impl Drop for Watching {
    fn drop(&mut self) {
        self.shared.remove(self.id);
    }
}
