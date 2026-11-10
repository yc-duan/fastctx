//! The tables of the content database.
//!
//! `frames` holds what was appended, as it was appended; `items` is derived
//! from it, one row an item at its latest, clustered in each stream's order
//! so a page is one range. `object_refs` counts, per scope, the records that
//! refer to an object: it answers whether a scope may read an object, and
//! what is left unreferenced when a scope's records go.

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS scopes (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS streams (
    id INTEGER PRIMARY KEY,
    scope INTEGER NOT NULL,
    name TEXT NOT NULL,
    generation INTEGER NOT NULL DEFAULT 0,
    next INTEGER NOT NULL DEFAULT 0,
    head BLOB,
    revision INTEGER NOT NULL DEFAULT 0,
    UNIQUE (scope, name)
);
CREATE TABLE IF NOT EXISTS frames (
    stream INTEGER NOT NULL,
    first INTEGER NOT NULL,
    count INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    head BLOB NOT NULL,
    bytes BLOB NOT NULL,
    PRIMARY KEY (stream, first)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS items (
    stream INTEGER NOT NULL,
    place BLOB NOT NULL,
    item BLOB NOT NULL,
    sequence INTEGER NOT NULL,
    body BLOB NOT NULL,
    objects BLOB NOT NULL,
    PRIMARY KEY (stream, place, item)
) WITHOUT ROWID;
CREATE UNIQUE INDEX IF NOT EXISTS items_by_identity ON items (stream, item);
CREATE TABLE IF NOT EXISTS object_refs (
    scope INTEGER NOT NULL,
    hash BLOB NOT NULL,
    size INTEGER NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (scope, hash)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS checkpoints (
    name TEXT PRIMARY KEY,
    state BLOB NOT NULL
) WITHOUT ROWID;
";
