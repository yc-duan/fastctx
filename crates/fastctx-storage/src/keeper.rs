//! One process at a time keeps a database: the one that opened it, until its
//! last connection has closed. Handles of the same process share its keeping;
//! checking a database before it opens, setting a damaged one aside and
//! salvaging it are done only by a sole keeper, when no other connection to
//! it is open anywhere: never under another process, nor under a handle of
//! this one that is still open or closing.

use crate::{Error, ErrorKind, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// How long an open waits for another process to let the database go.
const WAIT: Duration = Duration::from_secs(10);

/// The databases this process keeps, by path.
static KEPT: Mutex<Option<HashMap<PathBuf, Weak<Keeper>>>> = Mutex::new(None);

/// Held while any connection of this process to the database is open.
pub(crate) struct Keeper {
    _lock: fastctx_platform::lock::FileLock,
}

pub(crate) struct Kept {
    pub keeper: Arc<Keeper>,
    /// No other connection to the database is open: the open may check it
    /// and set it aside.
    pub sole: bool,
}

/// Keeps the database at `path` for this process, sharing the keeping of a
/// handle already open here; busy when another process keeps it.
pub(crate) fn keep(path: &Path) -> Result<Kept> {
    let file = lock_file(path);
    let began = Instant::now();
    loop {
        {
            let mut kept = KEPT.lock().expect("kept databases");
            let kept = kept.get_or_insert_with(HashMap::new);
            kept.retain(|_, keeper| keeper.strong_count() > 0);
            if let Some(keeper) = kept.get(path).and_then(Weak::upgrade) {
                return Ok(Kept { keeper, sole: false });
            }
            if let Some(lock) = fastctx_platform::lock::FileLock::try_acquire(&file)? {
                let keeper = Arc::new(Keeper { _lock: lock });
                kept.insert(path.to_path_buf(), Arc::downgrade(&keeper));
                return Ok(Kept { keeper, sole: true });
            }
        }
        if began.elapsed() >= WAIT {
            return Err(Error::with_kind(ErrorKind::Busy, format!("{} is in use by another process", path.display())));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub(crate) fn wait_closed(path: &Path, within: Duration) -> Result<()> {
    let deadline = Instant::now() + within;
    loop {
        let open = KEPT
            .lock()
            .expect("kept databases")
            .as_ref()
            .and_then(|kept| kept.get(path))
            .is_some_and(|kept| kept.strong_count() > 0);
        if !open {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Error::with_kind(ErrorKind::Busy, "database connections are still closing; deletion deferred"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn lock_file(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push("-keeper");
    path.with_file_name(name)
}
