//! What copying here read of files (`fastctx_hands::copy::Known`), kept
//! between the tries of a copy that goes on after a break
//! (`world-network.md`, 断开不是结束), so that a try reads again only what
//! changed: the files of a source, those already at a destination, and
//! what arrived of a file before the break.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fastctx_hands::copy::Known;

/// What is known under each path copied from or to.
#[derive(Default)]
pub(crate) struct Learned(Mutex<HashMap<PathBuf, Kept>>);

struct Kept {
    known: Arc<Mutex<Known>>,
    /// The tries holding it now.
    holding: usize,
    /// When the last of them let it go.
    since: Instant,
}

/// How long what was learned under a path is kept once no try holds it: as
/// long as a copy that broke takes to come back while its machines are
/// reachable. Its other end hears the connection gone within `SILENCE`, and
/// it waits at most `LONGEST` before it tries again. One that comes back
/// later reads its files again, once.
fn kept_for() -> Duration {
    fastctx_net::wire::SILENCE + fastctx_net::again::LONGEST
}

/// One try's hold on what is known under a path; let go when dropped.
pub(crate) struct Holding<'a> {
    learned: &'a Learned,
    path: PathBuf,
    pub(crate) known: Arc<Mutex<Known>>,
}

impl Learned {
    /// What is known under `path`, held by one try of a copy. It is locked
    /// only off the runtime's threads: a walk holds it while it reads.
    pub(crate) fn hold(&self, path: &Path) -> Holding<'_> {
        let mut kept = self.0.lock().expect("never poisoned");
        kept.retain(|_, kept| kept.holding > 0 || kept.since.elapsed() < kept_for());
        let entry = kept.entry(path.to_path_buf()).or_insert_with(|| Kept {
            known: Arc::default(),
            holding: 0,
            since: Instant::now(),
        });
        entry.holding += 1;
        Holding { learned: self, path: path.to_path_buf(), known: entry.known.clone() }
    }
}

impl Drop for Holding<'_> {
    fn drop(&mut self) {
        let mut kept = self.learned.0.lock().expect("never poisoned");
        if let Some(kept) = kept.get_mut(&self.path) {
            kept.holding -= 1;
            kept.since = Instant::now();
        }
    }
}
