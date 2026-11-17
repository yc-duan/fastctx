//! Resource limits and bounded lifecycle observations for startup composition.
use super::Record;
use std::{sync::Arc, time::Duration};

#[derive(Clone, Debug)]
pub struct Limits {
    pub elapsed: Duration,
    pub wal_growth_bytes: u64,
    /// Extra free space in addition to 120% of the logical database size.
    pub reserve_bytes: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            elapsed: Duration::from_secs(20 * 60),
            wal_growth_bytes: 512 * 1024 * 1024,
            reserve_bytes: 1024 * 1024 * 1024,
        }
    }
}

/// Observations are synchronous and must not block the worker. They expose no
/// database connection or write capability. SnapshotPinned precedes open permission.
#[derive(Clone, Debug)]
pub enum Event {
    SnapshotPinned(Record),
    ServicesReleased(Record),
    Progress { remaining_pages: i32, total_pages: i32 },
    CopyFlushed { pages: u32 },
    Copied(Record),
    IntegrityChecked(Record),
    Published(Record),
    Finished(Record),
    Warning(String),
}
pub type Observer = Arc<dyn Fn(Event) + Send + Sync>;
#[derive(Clone)]
pub struct Options {
    pub limits: Limits,
    pub observe: Observer,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            observe: Arc::new(|event| {
                if let Event::Warning(message) = event {
                    eprintln!("format upgrade: {message}");
                }
            }),
        }
    }
}
