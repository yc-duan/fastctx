//! The tables of a machine's asset library (`sync-v2-impl-design.md` §4.1).
//!
//! Every row of a scope's data carries the holding period it was written in:
//! reads, supply and export see the current period's rows alone, and a
//! revoked period's rows are purged by that number while a new period fills
//! in beside them (§4.5). Objects are known by hash and life: a hash that
//! comes back after its bytes were collected is a new life, with a file of
//! its own, which no earlier collector can touch (§4.3 I5).

/// The layout of the tables this build makes (`layout.rs`): changed with
/// every change to them until the layout is settled (plan 3-3).
pub(crate) const LAYOUT: &str = "2026-11-12.6";

/// Made before anything else as the library opens (`layout.rs`): where its
/// layout is stamped. Nothing is written when it is there already.
pub(crate) const PRELUDE: &str = "CREATE TABLE IF NOT EXISTS stamps (name TEXT PRIMARY KEY, value BLOB NOT NULL) WITHOUT ROWID;";

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS stamps (name TEXT PRIMARY KEY, value BLOB NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS scopes (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL UNIQUE,
    description BLOB,
    state TEXT NOT NULL CHECK (state IN ('provisional', 'live', 'disabled', 'purging')),
    holding INTEGER,
    -- The hub whose network the scope belongs to, on a device (empty in a
    -- hub's own libraries), and whether a hub's descriptor ever installed it.
    hub TEXT NOT NULL DEFAULT '',
    described INTEGER NOT NULL DEFAULT 0 CHECK (described IN (0, 1)),
    -- The holding period every stream of the scope was last pulled whole
    -- in, on a device: until it is the current one, the scope is asked
    -- after by itself, not by the change numbers alone (design §28.1).
    pulled INTEGER,
    -- What the hub last said this device may hold of the scope, on a
    -- device: changed, the scope is pulled whole again (HH04).
    allowed TEXT
);
CREATE INDEX IF NOT EXISTS scopes_of_hub ON scopes (hub) WHERE holding IS NOT NULL;
CREATE TABLE IF NOT EXISTS holdings (
    scope INTEGER NOT NULL,
    holding INTEGER NOT NULL,
    started INTEGER NOT NULL,
    ended INTEGER,
    acl_revision INTEGER NOT NULL,
    PRIMARY KEY (scope, holding)
) WITHOUT ROWID;
-- Where a device stands in each owner library of each hub: the library's
-- generation and the change number it caught up to (changes.rs).
CREATE TABLE IF NOT EXISTS feeds (
    hub TEXT NOT NULL,
    owner TEXT NOT NULL,
    generation TEXT NOT NULL,
    change INTEGER NOT NULL,
    at INTEGER NOT NULL,
    PRIMARY KEY (hub, owner)
) WITHOUT ROWID;
-- The networks a device holds scopes of: whether it is on each, and the
-- term it is on it under (networks.rs).
CREATE TABLE IF NOT EXISTS networks (
    hub TEXT PRIMARY KEY,
    term INTEGER NOT NULL,
    joined INTEGER NOT NULL CHECK (joined IN (0, 1))
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS streams (
    id INTEGER PRIMARY KEY,
    scope INTEGER NOT NULL,
    name TEXT NOT NULL,
    epoch INTEGER NOT NULL DEFAULT 0,
    next INTEGER NOT NULL DEFAULT 0,
    head BLOB,
    complete_through INTEGER,
    -- Where the hub last rejected the stream and in which epoch: until the
    -- stream is in a later epoch, what waits needs a new one first.
    rejected_at INTEGER,
    rejected_epoch INTEGER,
    -- How often the stream's index took in what is older than where readers
    -- stood — a window, a backfill merged, records indexed again — so they
    -- read it anew (`view.rs`).
    rebased INTEGER NOT NULL DEFAULT 0,
    UNIQUE (scope, name)
);
CREATE TABLE IF NOT EXISTS epochs (
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    descriptor BLOB NOT NULL,
    PRIMARY KEY (stream, epoch)
) WITHOUT ROWID;
-- The hub's signed epoch descriptors and gap facts a holder took (§8).
CREATE TABLE IF NOT EXISTS known_epochs (
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    predecessor INTEGER,
    fact BLOB NOT NULL,
    signature BLOB NOT NULL,
    PRIMARY KEY (stream, epoch)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS known_epochs_by_predecessor ON known_epochs (stream, predecessor);
CREATE TABLE IF NOT EXISTS known_gaps (
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    from_sequence INTEGER NOT NULL,
    fact BLOB NOT NULL,
    signature BLOB NOT NULL,
    PRIMARY KEY (stream, from_sequence)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS frames (
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    first INTEGER NOT NULL,
    holding INTEGER NOT NULL,
    count INTEGER NOT NULL,
    head BLOB NOT NULL,
    header BLOB NOT NULL,
    signature BLOB NOT NULL,
    body BLOB NOT NULL,
    change INTEGER,
    -- On a hub, the library's change number the frame was applied at.
    feed INTEGER,
    PRIMARY KEY (stream, epoch, first, holding)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS frame_objects (
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    first INTEGER NOT NULL,
    holding INTEGER NOT NULL,
    hash BLOB NOT NULL,
    PRIMARY KEY (stream, epoch, first, holding, hash)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS frame_objects_by_hash ON frame_objects (hash);
-- A hub keeps frames it stored but has not applied, in the holding period
-- it took them in, and their objects: they hold the bytes (I2) and give no
-- reading (P01). A refused frame's objects stay held, unreadable, while the
-- period lasts, for its producer's records sent again (§6.4). Empty on
-- devices.
CREATE TABLE IF NOT EXISTS parked (
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    first INTEGER NOT NULL,
    holding INTEGER NOT NULL,
    header BLOB NOT NULL,
    signature BLOB NOT NULL,
    body BLOB NOT NULL,
    sequence INTEGER NOT NULL,
    PRIMARY KEY (stream, epoch, first, holding)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS parked_objects (
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    first INTEGER NOT NULL,
    holding INTEGER NOT NULL,
    hash BLOB NOT NULL,
    PRIMARY KEY (stream, epoch, first, holding, hash)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS parked_objects_by_hash ON parked_objects (hash);
CREATE TABLE IF NOT EXISTS refused_objects (
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    first INTEGER NOT NULL,
    holding INTEGER NOT NULL,
    hash BLOB NOT NULL,
    PRIMARY KEY (stream, epoch, first, holding, hash)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS refused_objects_by_hash ON refused_objects (hash);
-- A predecessor's frame a hub found after a handover cut its epoch, kept
-- as it came, outside the stream (§8); its objects are held under its
-- holder. Empty on devices.
CREATE TABLE IF NOT EXISTS evidence (
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    first INTEGER NOT NULL,
    digest BLOB NOT NULL,
    holding INTEGER NOT NULL,
    holder TEXT NOT NULL,
    header BLOB NOT NULL,
    signature BLOB NOT NULL,
    body BLOB NOT NULL,
    why TEXT NOT NULL,
    at INTEGER NOT NULL,
    PRIMARY KEY (stream, epoch, first, digest, holding)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS items (
    stream INTEGER NOT NULL,
    place BLOB NOT NULL,
    item BLOB NOT NULL,
    revision INTEGER NOT NULL,
    state INTEGER NOT NULL CHECK (state IN (0, 1, 2)),
    kind INTEGER NOT NULL,
    schema INTEGER NOT NULL,
    body BLOB,
    objects BLOB,
    holding INTEGER NOT NULL,
    PRIMARY KEY (stream, place, item)
) WITHOUT ROWID;
CREATE UNIQUE INDEX IF NOT EXISTS items_by_identity ON items (stream, item);
CREATE INDEX IF NOT EXISTS items_floors ON items (stream, revision) WHERE state = 2;
CREATE INDEX IF NOT EXISTS items_by_revision ON items (stream, holding, revision);
CREATE TABLE IF NOT EXISTS object_refs (
    scope INTEGER NOT NULL,
    holding INTEGER NOT NULL,
    stream INTEGER NOT NULL,
    hash BLOB NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (scope, holding, stream, hash)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS object_refs_by_hash ON object_refs (hash);
CREATE TABLE IF NOT EXISTS objects (
    hash BLOB NOT NULL,
    life INTEGER NOT NULL,
    size INTEGER NOT NULL,
    state INTEGER NOT NULL CHECK (state IN (0, 1, 2)),
    placed INTEGER NOT NULL,
    unreferenced INTEGER,
    chunk INTEGER NOT NULL,
    PRIMARY KEY (hash, life)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS objects_by_state ON objects (state, placed);
CREATE TABLE IF NOT EXISTS wanted (
    hash BLOB PRIMARY KEY,
    size INTEGER NOT NULL,
    role TEXT NOT NULL,
    priority INTEGER NOT NULL,
    hint TEXT
) WITHOUT ROWID;
-- Which scope's records want which object (§4.3): a scope's wanted objects
-- are found from its own rows, never by scanning every wanted object.
CREATE TABLE IF NOT EXISTS wanted_in (
    scope INTEGER NOT NULL,
    hash BLOB NOT NULL,
    PRIMARY KEY (scope, hash)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS wanted_in_by_hash ON wanted_in (hash);
CREATE TABLE IF NOT EXISTS object_chunks (
    hash BLOB NOT NULL,
    first INTEGER NOT NULL,
    digests BLOB NOT NULL,
    PRIMARY KEY (hash, first)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS leases (
    id INTEGER PRIMARY KEY,
    hash BLOB NOT NULL,
    holder TEXT NOT NULL,
    expires INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS leases_by_hash ON leases (hash, expires);
CREATE TABLE IF NOT EXISTS transfers (
    id INTEGER PRIMARY KEY,
    hash BLOB NOT NULL,
    size INTEGER NOT NULL,
    chunk INTEGER NOT NULL,
    part TEXT NOT NULL,
    ranges BLOB NOT NULL,
    manifest BLOB NOT NULL,
    source TEXT
);
CREATE TABLE IF NOT EXISTS hub_receipts (
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    first INTEGER NOT NULL,
    digest BLOB NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('parked', 'applied', 'rejected')),
    standing BLOB NOT NULL,
    change INTEGER,
    parked INTEGER,
    signature BLOB NOT NULL,
    checkpointed INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (stream, epoch, first, digest, kind, standing)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS hub_receipt_sessions (
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    first INTEGER NOT NULL,
    digest BLOB NOT NULL,
    kind TEXT NOT NULL,
    standing BLOB NOT NULL,
    session INTEGER NOT NULL,
    PRIMARY KEY (stream, epoch, first, digest, kind, standing, session)
) WITHOUT ROWID;
-- A hub that holds a scope again keeps, for its kinds' rules, the state the
-- applied prefix of this period gives an item a revision floor still hides
-- (§2.4, §11): what a record is judged against. Empty on devices.
CREATE TABLE IF NOT EXISTS prefix_items (
    stream INTEGER NOT NULL,
    item BLOB NOT NULL,
    holding INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    place BLOB NOT NULL,
    state INTEGER NOT NULL,
    kind INTEGER NOT NULL,
    schema INTEGER NOT NULL,
    body BLOB,
    objects BLOB,
    PRIMARY KEY (stream, item, holding)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS cursors (
    stream INTEGER NOT NULL,
    holding INTEGER NOT NULL,
    sequence INTEGER NOT NULL,
    head BLOB,
    PRIMARY KEY (stream, holding)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS retention (
    stream INTEGER PRIMARY KEY,
    floor INTEGER NOT NULL,
    head BLOB
);
CREATE TABLE IF NOT EXISTS coverage (
    scope INTEGER NOT NULL,
    low BLOB NOT NULL,
    high BLOB NOT NULL,
    through INTEGER NOT NULL,
    PRIMARY KEY (scope, low)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS outbox (
    stream INTEGER PRIMARY KEY,
    uploaded INTEGER NOT NULL
);
-- Streams this device may hold no more in a holding period: nothing is
-- committed to them until the hub gives them back.
CREATE TABLE IF NOT EXISTS withheld (
    stream INTEGER NOT NULL,
    holding INTEGER NOT NULL,
    PRIMARY KEY (stream, holding)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS ops (
    op TEXT PRIMARY KEY,
    digest BLOB NOT NULL,
    kind TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('durable', 'refused', 'dropped')),
    delivery TEXT,
    kept BLOB,
    why TEXT
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS op_records (
    scope INTEGER NOT NULL,
    stream TEXT NOT NULL,
    revision INTEGER NOT NULL,
    op TEXT NOT NULL,
    PRIMARY KEY (scope, stream, revision)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS op_records_by_op ON op_records (op);
-- Kinds of a frame kept out of the index, against the frame's head: a
-- frame put in its place later is another one.
CREATE TABLE IF NOT EXISTS pending_schema (
    stream INTEGER NOT NULL,
    kind INTEGER NOT NULL,
    schema INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    first INTEGER NOT NULL,
    head BLOB NOT NULL,
    PRIMARY KEY (stream, epoch, first, head, kind, schema)
) WITHOUT ROWID;
-- Records judged and refused where their frame stands (`judged`), by their
-- place in the frame whose head this is: kept out of the index however it
-- is built.
CREATE TABLE IF NOT EXISTS refused_records (
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    first INTEGER NOT NULL,
    head BLOB NOT NULL,
    offset INTEGER NOT NULL,
    PRIMARY KEY (stream, epoch, first, head, offset)
) WITHOUT ROWID;
-- Items a record of which is kept out of the index — its kind unknown, or
-- waiting behind such a one: a record of the item that comes after waits
-- too, so every holder judges an item's records in their order (V01). Of
-- the holding period whose frames keep that record: another period's does
-- not hold this one's back (X01).
CREATE TABLE IF NOT EXISTS pending_items (
    stream INTEGER NOT NULL,
    holding INTEGER NOT NULL,
    item BLOB NOT NULL,
    PRIMARY KEY (stream, holding, item)
) WITHOUT ROWID;
-- Records of a kind known here kept out of the index behind an earlier one
-- of their item still out, by their place in the frame whose head this is.
CREATE TABLE IF NOT EXISTS deferred_records (
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    first INTEGER NOT NULL,
    head BLOB NOT NULL,
    offset INTEGER NOT NULL,
    PRIMARY KEY (stream, epoch, first, head, offset)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS purges (
    scope INTEGER NOT NULL,
    holding INTEGER NOT NULL,
    phase TEXT NOT NULL,
    position BLOB,
    PRIMARY KEY (scope, holding)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS published (
    hash BLOB NOT NULL,
    life INTEGER NOT NULL,
    at INTEGER NOT NULL,
    PRIMARY KEY (hash, life)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS checkpoints (
    name TEXT PRIMARY KEY,
    state BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS conflicts (
    id INTEGER PRIMARY KEY,
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    first INTEGER NOT NULL,
    why TEXT NOT NULL,
    bytes BLOB NOT NULL,
    at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS resubmits (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    stream INTEGER NOT NULL,
    rejected INTEGER NOT NULL,
    position INTEGER NOT NULL,
    holding INTEGER NOT NULL,
    -- The epoch the stream was in when the hub rejected the run.
    epoch INTEGER NOT NULL,
    -- The record's place in the order it was first written in, kept when
    -- it is committed again and rejected again.
    seq INTEGER NOT NULL DEFAULT 0,
    record BLOB NOT NULL,
    op TEXT
);
-- Where a record committed again landed, with its place in that order.
CREATE TABLE IF NOT EXISTS resubmit_order (
    stream INTEGER NOT NULL,
    holding INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    PRIMARY KEY (stream, holding, revision)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS resubmits_by_run ON resubmits (stream, rejected);
CREATE TABLE IF NOT EXISTS object_holds (
    kind TEXT NOT NULL CHECK (kind IN ('resubmit', 'refused', 'evidence')),
    holder TEXT NOT NULL,
    hash BLOB NOT NULL,
    proven INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (kind, holder, hash)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS object_holds_by_hash ON object_holds (hash);
CREATE TABLE IF NOT EXISTS migrations (
    kind TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    position BLOB,
    count INTEGER NOT NULL DEFAULT 0,
    digest BLOB
) WITHOUT ROWID;
";

/// The bytes of the small objects, a database of their own so the asset
/// library's pages hold items and frames.
pub(crate) const BYTES_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS object_bytes (
    hash BLOB NOT NULL,
    life INTEGER NOT NULL,
    bytes BLOB NOT NULL,
    PRIMARY KEY (hash, life)
) WITHOUT ROWID;
";

/// What a hub's owner library keeps besides (§6.2, §6.5, §17): the change
/// log that orders the applied frames of each scope, the frames kept but
/// not yet applied with each scope's parked sequence and accumulator (whose
/// every value is kept for good, so a parked receipt can always be
/// checked), the frames refused, and where each device stands.
pub(crate) const HUB_SCHEMA: &str = "
-- The frames applied after a change number of the library, for what a
-- device has yet to take (standing.rs, design §28.9).
CREATE INDEX IF NOT EXISTS frames_by_feed ON frames (feed) WHERE feed IS NOT NULL;
-- Each stream's moves to another machine (design §28.10): the epoch it was
-- handed over from and the one it was handed over in, to which machine and
-- device key, and by what — a release or an adoption.
CREATE TABLE IF NOT EXISTS moves (
    stream INTEGER NOT NULL,
    predecessor INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    machine TEXT NOT NULL,
    successor BLOB NOT NULL,
    basis TEXT NOT NULL CHECK (basis IN ('release', 'adopt')),
    at INTEGER NOT NULL,
    PRIMARY KEY (stream, predecessor)
) WITHOUT ROWID;
-- Each stream's latest change under the library's own change number, one
-- row a stream: what a device asks after where it stands (changes.rs).
CREATE TABLE IF NOT EXISTS stream_changes (
    stream INTEGER PRIMARY KEY,
    change INTEGER NOT NULL,
    class INTEGER NOT NULL,
    next INTEGER NOT NULL,
    head BLOB NOT NULL,
    bytes INTEGER NOT NULL,
    remade INTEGER NOT NULL DEFAULT 0,
    at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS stream_changes_by_change ON stream_changes (change);
CREATE INDEX IF NOT EXISTS stream_changes_by_class ON stream_changes (class, change);
CREATE TABLE IF NOT EXISTS change_log (
    scope INTEGER NOT NULL,
    change INTEGER NOT NULL,
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    first INTEGER NOT NULL,
    prefix BLOB NOT NULL,
    PRIMARY KEY (scope, change)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS hub_epochs (
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    device BLOB NOT NULL,
    key BLOB NOT NULL,
    start INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('source', 'observed')),
    predecessor INTEGER,
    follows BLOB NOT NULL,
    fact BLOB NOT NULL,
    signature BLOB NOT NULL,
    PRIMARY KEY (stream, epoch)
) WITHOUT ROWID;
CREATE UNIQUE INDEX IF NOT EXISTS hub_epochs_by_predecessor ON hub_epochs (stream, predecessor);
CREATE TABLE IF NOT EXISTS gaps (
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    from_sequence INTEGER NOT NULL,
    to_sequence INTEGER NOT NULL,
    follows BLOB NOT NULL,
    fact BLOB NOT NULL,
    signature BLOB NOT NULL,
    PRIMARY KEY (stream, epoch, from_sequence)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS parked_history (
    scope INTEGER NOT NULL,
    sequence INTEGER NOT NULL,
    accumulated BLOB NOT NULL,
    PRIMARY KEY (scope, sequence)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS refused_frames (
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    first INTEGER NOT NULL,
    digest BLOB NOT NULL,
    why TEXT NOT NULL,
    signature BLOB NOT NULL,
    PRIMARY KEY (stream, epoch, first)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS device_cursors (
    device BLOB NOT NULL,
    stream INTEGER NOT NULL,
    holding INTEGER NOT NULL,
    sequence INTEGER NOT NULL,
    PRIMARY KEY (device, stream, holding)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS intents (
    id TEXT PRIMARY KEY,
    body BLOB NOT NULL,
    done BLOB NOT NULL
) WITHOUT ROWID;
";
