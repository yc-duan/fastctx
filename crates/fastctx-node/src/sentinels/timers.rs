//! Sentinel wake-ups ride the node's durable timer table. A firing goes to the
//! sentinel runtime and fires again until the runtime acknowledges that very one.
use crate::Node;
use crate::timers::store::Store;
use fastctx_agent::FutureResult;
use fastctx_sentinel::ports::{Fired, Timer, Timers};
use fastctx_types::timers::{Action, Owner, Timer as Row};
use jiff::Timestamp;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::broadcast;

/// A firing still unacknowledged after this long is delivered again.
const UNACKNOWLEDGED: Duration = Duration::from_secs(300);

/// Firings on their way to the sentinel runtime.
pub(crate) struct Firings(broadcast::Sender<Fired>);
impl Default for Firings {
    fn default() -> Self {
        Self(broadcast::channel(256).0)
    }
}

fn sentinel_timer(row: &Row) -> Option<Timer> {
    let (Owner::Sentinel { id: owner }, Action::Sentinel { action }) = (&row.owner, &row.action) else { return None };
    Some(Timer { id: row.id.clone(), owner: owner.clone(), at: row.due.as_millisecond(), action: action.clone() })
}

/// The timer table's effect for a sentinel action: hand the firing over, and
/// keep it due again in case the runtime never acknowledges it.
pub(crate) fn fire(node: &Node, store: &Store, timer: &Row) -> Result<(), String> {
    let Some(row) = store.begin(timer, Timestamp::now()).map_err(|error| error.to_string())? else { return Ok(()) };
    let fired = Fired {
        timer: sentinel_timer(&row).ok_or("the timer is not a sentinel's")?,
        scheduled_at: row.due.as_millisecond(),
        late_ms: row.late_ms().max(0) as u64,
    };
    node.sentinels.firings.0.send(fired).map_err(|_| "no sentinel runtime is listening")?;
    let until = Timestamp::now().checked_add(UNACKNOWLEDGED).map_err(|error| error.to_string())?;
    store.hold(&row, until).map_err(|error| error.to_string())
}

pub(crate) struct SentinelTimers(pub Weak<Node>);
impl SentinelTimers {
    fn node(&self) -> Result<Arc<Node>, String> {
        self.0.upgrade().ok_or_else(|| "the sentinel node has stopped".into())
    }
    fn store(&self) -> Result<Arc<Store>, String> {
        let node = self.node()?;
        crate::timers::table(&node).cloned().map_err(|error| error.message)
    }
}
impl Timers for SentinelTimers {
    fn subscribe(&self) -> FutureResult<'_, broadcast::Receiver<Fired>> {
        Box::pin(async move { Ok(self.node()?.sentinels.firings.0.subscribe()) })
    }
    fn acknowledge<'a>(&'a self, fired: &'a Fired) -> FutureResult<'a, ()> {
        Box::pin(async move {
            let store = self.store()?;
            if let Some(row) = store.get(&fired.timer.id)
                && sentinel_timer(&row).as_ref() == Some(&fired.timer)
                && row.due.as_millisecond() == fired.scheduled_at
            {
                store.complete(&row).map_err(|error| error.to_string())?;
            }
            Ok(())
        })
    }
    fn set<'a>(&'a self, timer: &'a Timer) -> FutureResult<'a, ()> {
        Box::pin(async move {
            let store = self.store()?;
            let owner = Owner::Sentinel { id: timer.owner.clone() };
            if store.get(&timer.id).is_some_and(|row| row.owner != owner) {
                return Err(format!("timer {} belongs to another owner", timer.id));
            }
            let due = Timestamp::from_millisecond(timer.at).map_err(|error| error.to_string())?;
            let action = Action::Sentinel { action: timer.action.clone() };
            store
                .set(timer.id.clone(), owner, due, action, crate::speech::new_id("timer"))
                .map_err(|error| error.to_string())?;
            Ok(())
        })
    }
    fn cancel<'a>(&'a self, id: &'a str) -> FutureResult<'a, ()> {
        Box::pin(async move {
            let store = self.store()?;
            if store.get(id).is_some_and(|row| matches!(row.owner, Owner::Sentinel { .. })) {
                store.cancel(id).map_err(|error| error.to_string())?;
            }
            Ok(())
        })
    }
    fn list<'a>(&'a self, owner: &'a str) -> FutureResult<'a, Vec<Timer>> {
        Box::pin(async move {
            let owner = Owner::Sentinel { id: owner.into() };
            Ok(self.store()?.list(Some(&owner)).iter().filter_map(sentinel_timer).collect())
        })
    }
}
