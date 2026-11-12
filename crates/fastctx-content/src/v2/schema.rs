//! The tables of a machine's asset library (`sync-v2-impl-design.md` §4.1).
//!
//! Every row of a scope's data carries the holding period it was written in:
//! reads, supply and export see the current period's rows alone, and a
//! revoked period's rows are purged by that number while a new period fills
//! in beside them (§4.5). Objects are known by hash and life: a hash that
//! comes back after its bytes were collected is a new life, with a file of
//! its own, which no earlier collector can touch (§4.3 I5).

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS scopes (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL UNIQUE,
    description BLOB,
    state TEXT NOT NULL CHECK (state IN ('provisional', 'live', 'disabled', 'purging')),
    holding INTEGER
);
CREATE TABLE IF NOT EXISTS holdings (
    scope INTEGER NOT NULL,
    holding INTEGER NOT NULL,
    started INTEGER NOT NULL,
    ended INTEGER,
    acl_revision INTEGER NOT NULL,
    PRIMARY KEY (scope, holding)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS streams (
    id INTEGER PRIMARY KEY,
    scope INTEGER NOT NULL,
    name TEXT NOT NULL,
    epoch INTEGER NOT NULL DEFAULT 0,
    next INTEGER NOT NULL DEFAULT 0,
    head BLOB,
    complete_through INTEGER,
    UNIQUE (scope, name)
);
CREATE TABLE IF NOT EXISTS epochs (
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    descriptor BLOB NOT NULL,
    PRIMARY KEY (stream, epoch)
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
CREATE TABLE IF NOT EXISTS object_refs (
    scope INTEGER NOT NULL,
    holding INTEGER NOT NULL,
    hash BLOB NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (scope, holding, hash)
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
    heard_in INTEGER NOT NULL,
    checkpointed INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (stream, epoch, first, digest, kind, standing)
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
CREATE TABLE IF NOT EXISTS pending_schema (
    stream INTEGER NOT NULL,
    kind INTEGER NOT NULL,
    schema INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    first INTEGER NOT NULL,
    PRIMARY KEY (stream, kind, schema, epoch, first)
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
    stream INTEGER NOT NULL,
    rejected INTEGER NOT NULL,
    position INTEGER NOT NULL,
    holding INTEGER NOT NULL,
    record BLOB NOT NULL,
    op TEXT,
    PRIMARY KEY (stream, rejected, position)
) WITHOUT ROWID;
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
CREATE TABLE IF NOT EXISTS change_log (
    scope INTEGER NOT NULL,
    change INTEGER NOT NULL,
    stream INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    first INTEGER NOT NULL,
    prefix BLOB NOT NULL,
    PRIMARY KEY (scope, change)
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
