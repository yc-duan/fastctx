//! Folding a database's log into it beside its writer (design §28.43): a
//! commit does not wait for a checkpoint. SQLite's own folds the log inside
//! the commit that takes it past its threshold, about one commit in a
//! hundred of a busy stream, and that commit waited twenty milliseconds more
//! for it. The writer's connection keeps a log hook instead: past the
//! threshold it wakes a thread of this database, which folds what it can
//! with its own connection without stopping the writer or any reader
//! (PASSIVE); past four times the threshold — a fold that cannot keep up —
//! the writer folds it itself, as SQLite's own would at the threshold. A
//! reader holding an old snapshot holds the log back from either, as it
//! would SQLite's own. The thread runs only while there is folding to do;
//! its connection is closed before it is gone, and the writer waits for it
//! to be gone before it lets the file go, unwinding too. The writer's
//! truncation when idle is as it was.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// How long the thread waits for more to fold before it ends.
const LINGER: Duration = Duration::from_secs(10);

#[derive(Default)]
struct State {
    wanted: bool,
    closed: bool,
    worker: Option<JoinHandle<()>>,
}

struct Shared {
    path: PathBuf,
    state: Mutex<State>,
    woken: Condvar,
}

impl Shared {
    fn want(self: &Arc<Self>) {
        let mut state = self.state.lock().expect("never poisoned");
        if state.closed {
            return;
        }
        state.wanted = true;
        if state.worker.is_some() {
            // Running, or leaving: one leaving starts another for this.
            self.woken.notify_one();
            return;
        }
        self.start(&mut state);
    }

    /// Starts a thread while `state` is held. Not started, the writer's own
    /// fold at four times the threshold still folds the log.
    fn start(self: &Arc<Self>, state: &mut State) {
        let shared = self.clone();
        state.worker = std::thread::Builder::new().name("sqlite-checkpoint".into()).spawn(move || shared.work()).ok();
    }

    fn work(self: Arc<Self>) {
        let connection = rusqlite::Connection::open_with_flags(
            &self.path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()
        .filter(|connection| {
            // The log is put on the disk before the database is written from
            // it, and the database after, as a commit in full would have it.
            connection.busy_timeout(Duration::ZERO).is_ok() && connection.pragma_update(None, "synchronous", "FULL").is_ok()
        });
        loop {
            let mut state = self.state.lock().expect("never poisoned");
            if !state.wanted && !state.closed {
                state = self.woken.wait_timeout(state, LINGER).expect("never poisoned").0;
            }
            if state.closed || !state.wanted {
                break;
            }
            state.wanted = false;
            drop(state);
            if let Some(connection) = &connection {
                // Busy — another fold under way, the writer's truncation —
                // is no matter: the next commit past the threshold asks again.
                let _ = connection.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| row.get::<_, i64>(0));
            }
        }
        // Closed before the thread is known to be gone: whoever waits for it
        // waits for the connection too.
        drop(connection);
        let mut state = self.state.lock().expect("never poisoned");
        if state.wanted && !state.closed {
            // Asked again while it was leaving.
            self.start(&mut state);
        } else {
            state.worker = None;
        }
    }
}

/// Ends the thread when the writer's connection lets its hook go.
struct Closing(Arc<Shared>);

impl Drop for Closing {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().expect("never poisoned");
        state.closed = true;
        self.0.woken.notify_one();
    }
}

/// Held by the writer's thread past its connection: dropped, it closes the
/// folding and waits for its thread to be gone, then lets `held` go — the
/// file's keeper, which says no other handle of this process has the file.
pub(crate) struct Ending<K> {
    shared: Arc<Shared>,
    held: Option<K>,
}

impl<K> Drop for Ending<K> {
    fn drop(&mut self) {
        loop {
            let worker = {
                let mut state = self.shared.state.lock().expect("never poisoned");
                state.closed = true;
                self.shared.woken.notify_one();
                state.worker.take()
            };
            let Some(worker) = worker else { break };
            let _ = worker.join();
        }
        drop(self.held.take());
    }
}

/// Folding left to SQLite, inside the commit past the threshold (`Shape::
/// fold_inside`): no hook and no thread, only `held` let go as the writer's
/// thread ends.
pub(crate) fn inside<K>(path: &Path, held: K) -> Ending<K> {
    let shared = Arc::new(Shared { path: path.to_path_buf(), state: Mutex::new(State::default()), woken: Condvar::new() });
    Ending { shared, held: Some(held) }
}

/// Puts the log hook on the writer's `connection` of the database at `path`,
/// folding past `threshold` pages beside it; what the writer's thread holds
/// until it ends, `held` let go after the folding thread is gone. An error
/// leaves the connection not to be used: SQLite may keep a hook whose state
/// was freed (`wal_hook` sets the hook before it hands over its closure).
pub(crate) fn install<K>(connection: &mut rusqlite::Connection, path: &Path, threshold: u32, held: K) -> rusqlite::Result<Ending<K>> {
    let shared = Arc::new(Shared { path: path.to_path_buf(), state: Mutex::new(State::default()), woken: Condvar::new() });
    let (closing, threshold) = (Closing(shared.clone()), i32::try_from(threshold.max(1)).unwrap_or(i32::MAX));
    let hard = threshold.saturating_mul(4);
    connection.wal_hook(Some(move |wal: &mut rusqlite::hooks::Wal, pages: std::ffi::c_int| {
        if pages >= hard {
            // An error here would report a commit that took place as failed:
            // a fold that does not happen now happens at the next one.
            let _ = wal.checkpoint_v2(rusqlite::wal::CheckpointMode::PASSIVE);
        } else if pages >= threshold {
            closing.0.want();
        }
        Ok(())
    }))?;
    Ok(Ending { shared, held: Some(held) })
}
