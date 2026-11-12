//! The second version's asset library of one machine (`sync-v2-impl-design.md`
//! §4): scopes held in holding periods, streams of signed frames, the index
//! of each item's latest revision, and the objects records refer to, whose
//! life is decided in one database by one writer.
//!
//! It stands beside the first version's content database until each kind of
//! asset moves over (§15), in files of its own.

mod checkpoints;
mod collect;
mod commit;
mod device;
mod exchange;
mod facts;
mod frame;
mod handover;
mod heads;
mod hub;
mod items;
mod kind;
mod letgo;
mod objects;
mod ops;
mod reads;
mod receipts;
mod rejected;
mod schema;
mod scopes;
mod stamps;
mod succession;
mod timeline;
mod transfers;
mod window;

pub use collect::{Collected, Started};
pub use commit::{Commit, Committed};
pub use exchange::{Holder, Imported, RawFrame, Verifier};
pub use frame::{FORMAT, Header, KindRecord, Signer, open};
pub use facts::{EpochFact, EpochKind, GapFact};
pub use handover::{Cutoff, Signed, Successor};
pub use heads::Head;
pub use kind::{Actor, Deletion, Holding, Import, Kind, Kinds, Write, Writer};
pub use timeline::{EVENT, OVERSIZED, Timeline};
pub use hub::{Authorizer, Drained, FrameKey, HubSigner, Received, Refused, Taking, applied_statement, parked_statement};
pub use ops::{Delivery, Op};
pub use reads::{Entry, PAGE};
pub use receipts::{Mark, Receipt, Report, Said};
pub use rejected::Resubmit;
pub use window::Pruned;
pub use scopes::Purged;
pub use succession::{ENDS_AT_ONCE, HubVerifier, Standing};
pub use objects::{CHUNK_BYTES, LEASE, Lease, READ_BYTES, ROW_BYTES};
pub use transfers::Transfer;

use crate::Result;
use fastctx_storage::Database;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The library's database within the directory it is opened in.
pub const FILE: &str = "assets.sqlite3";
/// The small objects' bytes.
pub const BYTES_FILE: &str = "asset-bytes.sqlite3";
/// The objects that are files, each `<hh>/<hex>.<life>`.
pub const OBJECTS_DIRECTORY: &str = "asset-objects";

/// One machine's asset library. Cloning it shares the library.
#[derive(Clone)]
pub struct Library(Arc<Inner>);

struct Inner {
    database: Database,
    bytes: Database,
    objects: PathBuf,
}

impl Library {
    /// Opens the library in `directory`, making it when it is not there, and
    /// settles what a stop left half done before any object is read or put
    /// (`collect::startup`). Call on a thread that may block.
    pub fn open(directory: &Path) -> Result<(Self, Started)> {
        Self::open_with(directory, schema::SCHEMA.to_string())
    }

    /// Opens an owner library of a hub (§6.2): the same library with the
    /// hub's own tables besides, caching at most 2 MiB of pages across its
    /// two databases' writers and readers, as up to 64 are open at once
    /// (R27).
    pub fn open_hub(directory: &Path) -> Result<(Self, Started)> {
        let shape =
            fastctx_storage::Shape { cache_bytes: Some(512 << 10), reader_cache_bytes: Some(32 << 10), ..Default::default() };
        Self::open_shaped(directory, format!("{}{}", schema::SCHEMA, schema::HUB_SCHEMA), shape)
    }

    fn open_with(directory: &Path, schema: String) -> Result<(Self, Started)> {
        Self::open_shaped(directory, schema, fastctx_storage::Shape::default())
    }

    fn open_shaped(directory: &Path, schema: String, shape: fastctx_storage::Shape) -> Result<(Self, Started)> {
        let database = Database::open_shaped(&directory.join(FILE), &schema, shape)?;
        let bytes = Database::open_shaped(&directory.join(BYTES_FILE), schema::BYTES_SCHEMA, shape)?;
        let library = Self(Arc::new(Inner { database, bytes, objects: directory.join(OBJECTS_DIRECTORY) }));
        let started = library.startup()?;
        Ok((library, started))
    }

    pub(crate) fn database(&self) -> &Database {
        &self.0.database
    }

    /// Whether a clone of this library is held anywhere but by this one.
    pub fn shared(&self) -> bool {
        Arc::strong_count(&self.0) > 1
    }
}
