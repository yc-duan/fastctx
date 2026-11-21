//! The second version's asset library of one machine (`sync-v2-impl-design.md`
//! §4): scopes held in holding periods, streams of signed frames, the index
//! of each item's latest revision, and the objects records refer to, whose
//! life is decided in one database by one writer.
//!
//! It stands beside the first version's content database until each kind of
//! asset moves over (§15), in files of its own.

mod changes;
mod checkpoints;
mod collect;
mod commit;
mod copies;
mod device;
mod document;
mod draft;
mod exchange;
mod facts;
mod frame;
mod handback;
mod handover;
mod heads;
mod hub;
mod hub_ordered;
mod items;
mod job;
mod judged;
mod kind;
mod layout;
mod ledger_entries;
mod letgo;
mod logs;
mod activity;
mod backfill;
mod aside;
mod artifact;
mod attachment;
mod file_changes;
mod meta;
mod names;
mod words;
mod moves;
mod networks;
mod objects;
mod ops;
mod pending;
mod reads;
mod receipts;
mod rejected;
mod schema;
mod scopes;
mod snapshot;
mod stamps;
mod sentinel_run;
mod timer;
mod standing;
mod succession;
mod timeline;
mod transfers;
mod unaccepted;
mod view;
mod vouch;
mod window;

pub use checkpoints::HeldItem;
pub use collect::{Collected, Started};
pub use commit::{Commit, Committed};
pub use copies::{CopySize, Dropped, HeldScope};
pub use vouch::{Able, Marked, Reclaimed, Settled, VOUCHED_FOR, Voucher, Vouchable, VouchableObject};
pub use exchange::{Holder, Imported, RawFrame, Verifier};
pub use frame::{FORMAT, Header, KindRecord, Signer, open};
pub use facts::{EpochFact, EpochKind, GapFact};
pub use handover::{Cutoff, EpochBounds, Signed, Successor};
pub use heads::Head;
pub use kind::{Actor, Deletion, Holding, Import, Kind, Kinds, Write, Writer};
pub use timeline::{EVENT, OVERSIZED, Timeline};
pub use activity::{AccountActivity, AccountUsage};
pub use artifact::{ConversationArtifact, part_name};
pub use draft::ConversationDraft;
pub use attachment::ConversationAttachment;
pub use file_changes::{ConversationFileChange, FILE_HOWS};
pub use sentinel_run::ConversationSentinelRun;
pub use timer::ConversationTimer;
pub use document::{
    AccountCollaboration, AccountNotice, AccountProfile, AccountSettings, AccountSkill, ConversationGoal, ConversationHistory,
    ConversationPlan, ConversationSentinel,
    MachineAudit, ProjectLedger,
};
pub use handback::{HandedBack, Tail, read_handed_back};
pub use hub_ordered::{HUB_ORDERED, hub_orders};
pub use names::item_of;
pub use logs::{HubLog, KEPT_MS, MachineLog, McpLog};
pub use job::{JobFacts, JobOutput, OUTPUT_PARTS};
pub use meta::{ConversationMeta, META_ITEMS};
pub use words::{ConversationWords, WORDS_STATES};
pub use hub::{Authorizer, Drained, FrameKey, HubSigner, Received, Refused, Taking, applied_statement, parked_statement};
pub use ops::{Delivery, OP_REUSED, Op};
pub use reads::{Entry, PAGE};
pub use receipts::{Mark, Receipt, Report, Said};
pub use rejected::Resubmit;
pub use window::Pruned;
pub use changes::{Changed, Changes};
pub use backfill::{Backfill, Backfilled};
pub use snapshot::{Covered, Installed, OLDER_BYTES, Older, OlderTaken, WINDOW_BYTES, WINDOW_ITEMS, Window};
pub use view::{ViewChange, ViewChanges, ViewPage, view_sequence};
pub use standing::{Fed, StreamBytes, Totals, running_logs};
pub use moves::{Authority, Begun, KeyCheck, Moved, Releasable, Release, Taken};
pub use networks::{Listing, Network};
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

/// Compacts the databases of the library in `directory`, none of them open
/// in any process: each by its file's name with what it came to, up to the
/// first that failed.
pub fn compact(directory: &Path) -> fastctx_storage::Result<Vec<(&'static str, fastctx_storage::Compacted)>> {
    [FILE, BYTES_FILE]
        .into_iter()
        .map(|name| match fastctx_storage::compact(&directory.join(name)) {
            Ok(compacted) => Ok((name, compacted)),
            Err(error) => Err(fastctx_storage::Error::with_kind(error.kind(), format!("{name}: {error}"))),
        })
        .collect()
}

/// One machine's asset library. Cloning it shares the library.
#[derive(Clone)]
pub struct Library(Arc<Inner>);

struct Inner {
    database: Database,
    bytes: Database,
    objects: PathBuf,
    /// Held to read by every write of an object's bytes and by every
    /// transfer, to write by a clearing (`clear_unrooted`): while a machine
    /// clears for a new holder, no write is under way and none begins.
    clearing: std::sync::RwLock<()>,
    /// The kinds this library knows, when it was told (`know`): a record of
    /// another, or of a later schema, stays in its frame and out of the
    /// index until it knows it (§2.3).
    kinds: std::sync::RwLock<Option<Arc<dyn Authorizer>>>,
    /// Whether it is a hub's owner library, with the hub's own tables.
    hub: bool,
    /// The scopes committed to here since `committed` was last asked: the
    /// ones with something of this device's own to upload.
    committed: std::sync::Mutex<std::collections::BTreeSet<String>>,
    /// Counts the writes that may move what a reader reads (`view.rs`).
    moved: tokio::sync::watch::Sender<u64>,
}

/// The page size a library is made with (§18, 3-6): a frame's body, up to a
/// MiB, is a long chain of pages, and with 4 KiB ones a replica took 24,000
/// records a second; with 16 KiB ones 49,000, its pages read faster too,
/// the rest as before; 64 KiB ones gave less. A library made earlier keeps
/// the size it was made with.
const PAGE_BYTES: u32 = 16 * 1024;

impl Library {
    /// Opens the library in `directory`, making it when it is not there, and
    /// settles what a stop left half done before any object is read or put
    /// (`collect::startup`). Call on a thread that may block.
    pub fn open(directory: &Path) -> Result<(Self, Started)> {
        let shape = fastctx_storage::Shape { page_bytes: Some(PAGE_BYTES), ..Default::default() };
        Self::open_shaped(directory, schema::SCHEMA.to_string(), shape, false)
    }

    /// Opens an owner library of a hub (§6.2): the same library with the
    /// hub's own tables besides, caching a few MiB of pages across its two
    /// databases' writers and readers, as up to 64 are open at once (R27):
    /// a writer the pages its trees are descended by, 64 of them, where
    /// with 32 every lookup read its way down from the file again.
    pub fn open_hub(directory: &Path) -> Result<(Self, Started)> {
        let shape = fastctx_storage::Shape {
            cache_bytes: Some(1 << 20),
            reader_cache_bytes: Some(64 << 10),
            page_bytes: Some(PAGE_BYTES),
            ..Default::default()
        };
        Self::open_shaped(directory, format!("{}{}{}", schema::SCHEMA, schema::HUB_SCHEMA, ledger_entries::SCHEMA), shape, true)
    }

    fn open_shaped(directory: &Path, schema: String, shape: fastctx_storage::Shape, hub: bool) -> Result<(Self, Started)> {
        let file = directory.join(FILE);
        let database = Database::open_initialized_shaped(&file, schema::PRELUDE, shape, |db| layout::made(db, &file, &schema))?;
        let bytes = Database::open_shaped(&directory.join(BYTES_FILE), schema::BYTES_SCHEMA, shape)?;
        let library = Self(Arc::new(Inner {
            database,
            bytes,
            objects: directory.join(OBJECTS_DIRECTORY),
            clearing: std::sync::RwLock::new(()),
            kinds: std::sync::RwLock::new(None),
            hub,
            committed: std::sync::Mutex::new(std::collections::BTreeSet::new()),
            moved: tokio::sync::watch::channel(0).0,
        }));
        let started = library.startup()?;
        Ok((library, started))
    }

    pub(crate) fn database(&self) -> &Database {
        &self.0.database
    }

    /// Something a reader reads may have moved: those waiting look again.
    pub(super) fn wrote(&self) {
        self.0.moved.send_modify(|count| *count = count.wrapping_add(1));
    }

    pub(super) fn watch_moves(&self) -> tokio::sync::watch::Receiver<u64> {
        self.0.moved.subscribe()
    }

    /// The scopes committed to here since this was last asked, as a commit
    /// of this device's own records went in: what may have something to
    /// upload. Kept in memory alone; after a start every scope is looked at.
    pub fn committed(&self) -> std::collections::BTreeSet<String> {
        std::mem::take(&mut *self.0.committed.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    /// Held while bytes of an object are written: no clearing comes between.
    pub(super) fn writing(&self) -> std::sync::RwLockReadGuard<'_, ()> {
        self.0.clearing.read().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Tells the library the kinds it knows: from now, a record of another
    /// kind, or of a later schema, that comes in a frame is kept out of the
    /// index until it does (`reconsider`).
    pub fn know(&self, kinds: Arc<dyn Authorizer>) {
        *self.0.kinds.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(kinds);
    }

    /// Whether records of `kind` at `schema` are known here: every one when
    /// the library was told no kinds.
    pub(super) fn knowing(&self) -> impl Fn(u16, u16) -> bool + Send + 'static {
        let kinds = self.0.kinds.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        move |kind, schema| kinds.as_ref().is_none_or(|kinds| kinds.knows(kind, schema))
    }

    /// The kinds this library knows, when it was told: what judges a
    /// record of them.
    pub(super) fn judging(&self) -> Option<Arc<dyn Authorizer>> {
        self.0.kinds.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Whether a clone of this library is held anywhere but by this one.
    pub fn shared(&self) -> bool {
        Arc::strong_count(&self.0) > 1
    }
}
