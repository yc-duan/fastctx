//! The hub's isolated heavy workers, reused by role for its lifetime.

use fastctx_platform::worker::{Pool, Role, Settings};
use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Worker roles linked into the executable before its command runtime starts.
pub const ROLES: &[Role] = &[];

// Bounds each heavy worker on the 2 GiB reference machine.
const MEMORY: u64 = 256 * 1024 * 1024;
// Keeps heavy work within the reference machine's two cores.
const PROCESSES: usize = 2;
// Reuses nearby requests while releasing idle memory after a minute.
const IDLE: Duration = Duration::from_secs(60);
// Bounds growth retained across repeated requests.
const RECYCLED_AFTER: u32 = 256;

pub(crate) struct Pools {
    program: PathBuf,
    pools: Mutex<HashMap<&'static str, Arc<Pool>>>,
}

impl Pools {
    pub(crate) fn new(program: PathBuf) -> Self {
        Self { program, pools: Mutex::default() }
    }
}

impl crate::Hub {
    /// Gets the shared pool of a linked role, creating it on first use.
    #[allow(dead_code, reason = "the hub's first worker callers are not registered yet")]
    pub(crate) fn worker(&self, role: &'static str) -> io::Result<Arc<Pool>> {
        if !ROLES.iter().any(|known| known.name == role) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "unknown hub worker role"));
        }
        let mut pools = self.workers.pools.lock().expect("worker pools");
        Ok(pools
            .entry(role)
            .or_insert_with(|| {
                Pool::new(Settings {
                    name: role,
                    program: self.workers.program.clone(),
                    role,
                    processes: PROCESSES,
                    memory: MEMORY,
                    recycled_after: RECYCLED_AFTER,
                    idle: IDLE,
                })
            })
            .clone())
    }
}
