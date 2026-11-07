//! Immutable catalog reads and a FIFO single writer. The writer transfers
//! exclusive ownership of its working state, not a mutex guard: durable
//! work never holds a shared lock, and readers always retain a committed
//! snapshot. Returning ownership admits the next edit.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio::sync::{mpsc, oneshot};

use crate::store::{Snapshot, Store};

pub(crate) struct Catalog {
    current: Arc<ArcSwap<Snapshot>>,
    edits: mpsc::Sender<oneshot::Sender<Editing>>,
}

pub(crate) struct Editing {
    store: Option<Box<Store>>,
    returned: Option<oneshot::Sender<Box<Store>>>,
}

impl Catalog {
    pub(crate) fn new(mut store: Store) -> std::io::Result<Self> {
        let current = Arc::new(ArcSwap::from_pointee(store.snapshot()));
        store.published = Some(current.clone());
        let (edits, mut receive) = mpsc::channel::<oneshot::Sender<Editing>>(256);
        let runtime = tokio::runtime::Handle::current();
        std::thread::Builder::new().name("hub-catalog".into()).spawn(move || {
            let _runtime = runtime.enter();
            let mut store = Box::new(store);
            while let Some(reply) = receive.blocking_recv() {
                let (returned, back) = oneshot::channel();
                let editing = Editing { store: Some(store), returned: Some(returned) };
                // Even a cancelled requester returns the owned state on drop.
                drop(reply.send(editing));
                match back.blocking_recv() {
                    Ok(kept) => store = kept,
                    Err(_) => return,
                }
            }
        })?;
        Ok(Self { current, edits })
    }

    pub(crate) fn reader(&self) -> Arc<ArcSwap<Snapshot>> {
        self.current.clone()
    }

    pub(crate) fn read(&self) -> Arc<Snapshot> {
        self.current.load_full()
    }

    /// Called only by blocking workers. Asynchronous coordinators dispatch
    /// the whole edit to such a worker before requesting ownership.
    pub(crate) fn edit(&self) -> Editing {
        let _waiting = fastctx_platform::doing::waiting_outside("catalog writer", std::time::Duration::from_secs(30));
        let (reply, receive) = oneshot::channel();
        self.edits.blocking_send(reply).expect("catalog writer running");
        receive.blocking_recv().expect("catalog writer running")
    }
}

impl Deref for Editing {
    type Target = Store;
    fn deref(&self) -> &Store {
        self.store.as_deref().expect("edit owns state")
    }
}
impl DerefMut for Editing {
    fn deref_mut(&mut self) -> &mut Store {
        self.store.as_deref_mut().expect("edit owns state")
    }
}
impl Drop for Editing {
    fn drop(&mut self) {
        if let (Some(mut store), Some(returned)) = (self.store.take(), self.returned.take()) {
            store.reset_committed();
            let _ = returned.send(store);
        }
    }
}

/// Copy-on-write catalog components. Cloning a snapshot copies only pointers;
/// the single writer copies a component only when it first changes it.
#[derive(Clone)]
pub struct Shared<T: Clone>(Arc<T>);
impl<T: Clone> From<T> for Shared<T> {
    fn from(value: T) -> Self {
        Self(Arc::new(value))
    }
}
impl<T: Clone> Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T: Clone> DerefMut for Shared<T> {
    fn deref_mut(&mut self) -> &mut T {
        Arc::make_mut(&mut self.0)
    }
}
impl<T: Clone + PartialEq> PartialEq for Shared<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
