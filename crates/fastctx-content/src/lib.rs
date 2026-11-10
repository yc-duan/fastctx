//! The content database of one machine: every scope's streams as append-only
//! logs of item states, and the index of each item's latest state.
//!
//! A stream has one producer, which appends batches of records. A batch is
//! kept as frames, its exact bytes as they go to other holders, and applied
//! to the index in the same transaction. Reading is one bounded range of the
//! index and writes nothing; whoever waits for a stream is told when it moves.

mod append;
mod audit;
mod changes;
mod error;
mod forget;
mod frame;
mod objects;
mod read;
mod reclaim;
mod record;
mod schema;
mod wake;

pub use append::{Appended, BATCH_BYTES, Batch, CHECKPOINT_BYTES, Checkpoint};
pub use audit::Audited;
pub use changes::{Change, Changes};
pub use error::{Error, Result};
pub use forget::Forgotten;
pub use objects::{OBJECTS_DIRECTORY, OBJECTS_FILE, Objects, READ_BYTES, ROW_BYTES};
pub use read::{Anchor, Item, Page, Place};
pub use reclaim::Reclaimed;
pub use record::{FRAME_BYTES, FRAME_RECORDS, KEY_BYTES, ObjectRef, RECORD_BYTES, RECORD_OBJECTS, Record, State};

use fastctx_storage::Database;
use std::path::Path;
use std::sync::Arc;

/// The database's file within the directory it is opened in.
pub const FILE: &str = "content.sqlite3";

/// An item is a few kilobytes: a page this large keeps one whole, where a
/// smaller page spills it over several (measured on the drill: a third less
/// time to import, and to read a page, than with 4 KiB pages).
const PAGE_BYTES: u32 = 16 * 1024;
/// What the writer keeps in memory: an import's pages stay there until its
/// commit instead of being written out and read back.
const CACHE_BYTES: u32 = 16 * 1024 * 1024;
/// The log is folded into the database after this many pages of it.
const CHECKPOINT_PAGES: u32 = 2_000;

/// The longest scope or stream name, in bytes.
pub const NAME_BYTES: usize = 256;

/// Names one stream: the scope it is seen and synchronized with, and its
/// name within that scope.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StreamKey {
    pub scope: String,
    pub name: String,
}

impl StreamKey {
    pub fn new(scope: impl Into<String>, name: impl Into<String>) -> Self {
        Self { scope: scope.into(), name: name.into() }
    }

    fn check(&self) -> Result<()> {
        for (what, value) in [("scope", &self.scope), ("stream name", &self.name)] {
            if value.is_empty() || value.len() > NAME_BYTES {
                return Err(Error::Invalid(format!("a {what} is 1 to {NAME_BYTES} bytes")));
            }
        }
        Ok(())
    }
}

/// One machine's content database. Cloning it shares the database.
#[derive(Clone)]
pub struct Content(Arc<Inner>);

struct Inner {
    database: Database,
    objects: Objects,
    wakes: wake::Wakes,
    reclaiming: std::sync::Mutex<reclaim::Passes>,
}

impl Content {
    /// Opens the content database in `directory`, making it when it is not
    /// there. Call on a thread that may block.
    pub fn open(directory: &Path) -> Result<Self> {
        let shape = fastctx_storage::Shape {
            page_bytes: Some(PAGE_BYTES),
            cache_bytes: Some(CACHE_BYTES),
            checkpoint_pages: Some(CHECKPOINT_PAGES),
        };
        let database = Database::open_shaped(&directory.join(FILE), schema::SCHEMA, shape)?;
        let objects = Objects::open(directory)?;
        Ok(Self(Arc::new(Inner {
            database,
            objects,
            wakes: wake::Wakes::default(),
            reclaiming: std::sync::Mutex::default(),
        })))
    }

    /// The objects the records here refer to.
    pub fn objects(&self) -> &Objects {
        &self.0.objects
    }

    /// The storage counters of the database, for readings.
    pub fn statistics(&self) -> fastctx_storage::Statistics {
        self.0.database.statistics()
    }
}
