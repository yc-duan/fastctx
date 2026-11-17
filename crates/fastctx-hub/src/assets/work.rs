//! How much asset work the hub does at once.
//!
//! An asset request is answered on a thread that may block, and spends most
//! of that time waiting its turn at its owner library's one writer. Unbounded,
//! the requests of members that left, or asked again on a new connection,
//! each kept a thread no one waited for: a network whose connections dropped
//! for a while had the hub working on what nobody would hear, with fewer
//! threads left for what its members asked next. So a request waits for a
//! turn before it takes a thread: so many of one member's are answered at
//! once, and so many of all members' together, the rest waiting in the order
//! they came, taking nothing while they wait; one whose asker is gone by
//! then is not answered at all (`control`).

use fastctx_types::MachineName;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Asset requests answered at once, every member's together: each holds a
/// thread while it is answered.
const AT_ONCE: usize = 48;
/// Asset requests of one member answered at once: its machines' turns at
/// one library's writer come one after another whatever it asks.
const OF_ONE: usize = 6;
/// How long a request on the control connection waits its turn before its
/// asker is told to ask again: less than an asker waits for its answer.
const WAITS: Duration = Duration::from_secs(40);
/// Members whose turns are kept track of before those with nothing asked
/// are forgotten.
const KEPT: usize = 4096;

pub(crate) struct Work {
    all: Arc<Semaphore>,
    each: Mutex<HashMap<MachineName, Arc<Semaphore>>>,
}

impl Default for Work {
    fn default() -> Self {
        Self { all: Arc::new(Semaphore::new(AT_ONCE)), each: Mutex::new(HashMap::new()) }
    }
}

/// A turn at the hub's asset work, held until it is dropped.
pub(crate) struct Turn {
    _each: OwnedSemaphorePermit,
    _all: OwnedSemaphorePermit,
}

impl Work {
    fn of(&self, member: &MachineName) -> Arc<Semaphore> {
        let mut each = self.each.lock().expect("asset work");
        if each.len() >= KEPT {
            each.retain(|_, one| Arc::strong_count(one) > 1);
        }
        each.entry(member.clone()).or_insert_with(|| Arc::new(Semaphore::new(OF_ONE))).clone()
    }

    /// `member`'s turn, waited for as long as it takes: for a line, where one
    /// exchange goes on at a time and the member waits on it.
    pub(crate) async fn wait(&self, member: &MachineName) -> Turn {
        let each = self.of(member).acquire_owned().await.expect("never closed");
        let all = self.all.clone().acquire_owned().await.expect("never closed");
        Turn { _each: each, _all: all }
    }

    /// `member`'s turn, or why none came in time.
    pub(crate) async fn turn(&self, member: &MachineName) -> Result<Turn, String> {
        tokio::time::timeout(WAITS, self.wait(member))
            .await
            .map_err(|_| "the hub has as much asset work as it takes at once; ask again shortly".to_string())
    }
}
