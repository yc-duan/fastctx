//! What the hub keeps and gives no device any more.
//!
//! Until beta.26 every machine of an account wrote what it did, ran and spent
//! for the account into the account's records scope (`activity`, `usage`,
//! `jobs`, `output`), and nodes mirrored their run logs and their MCP
//! commands' logs into scopes of their own. Nothing writes these since then,
//! and nothing reads a device's copy of them but a stale fallback for a
//! machine that is away. Yet every machine an account is logged in on, a new
//! one first, was given all of them to hold: months of history it never
//! reads, through the hub's line.
//!
//! The hub keeps them as they are and gives them to none: they are no row of
//! `Changes`, a withheld stream of `Streams` (a device lets its copy go,
//! keeping what it wrote and the hub never took), no stream `Frames` reads
//! from, no reason to reach an object, and no part of what a device lacks.
//! A scope retired whole is listed among what a device holds no more, which
//! ends its holding there. Writing to them is judged as before: an older
//! node that still writes is not refused into a loop of retries.

use fastctx_types::assets::{Readers, StreamClass};

use super::scopes::Scope;

/// The streams of an account's records no device holds any more.
const RECORDS: [&str; 4] = ["activity", "usage", "jobs", "output"];

/// Whether no stream of the scope is given to any device: a machine's run
/// log, the hub's own log, and what an account's MCP commands logged on a
/// machine.
pub(crate) fn whole(scope: &Scope) -> bool {
    let key = scope.descriptor.scope.as_str();
    match &scope.descriptor.readers {
        Readers::Working { .. } => true,
        Readers::Hub | Readers::Body { .. } => key.ends_with("/log"),
        _ => false,
    }
}

/// Whether `stream` of the scope is given to no device.
pub(crate) fn stream(scope: &Scope, stream: &str) -> bool {
    whole(scope) || (records(scope) && RECORDS.iter().any(|prefix| stream.starts_with(prefix)))
}

/// Whether the class `class` of the scope names only streams given to no
/// device: a listing of what a device holds leaves it out.
pub(crate) fn class(scope: &Scope, class: &StreamClass) -> bool {
    whole(scope) || (records(scope) && RECORDS.iter().any(|prefix| class.prefix.starts_with(prefix)))
}

/// Whether the scope keeps an account's records, or those of a machine
/// shared whole.
fn records(scope: &Scope) -> bool {
    match &scope.descriptor.readers {
        Readers::Account { .. } => true,
        Readers::Body { .. } => scope.descriptor.scope.ends_with("/records"),
        _ => false,
    }
}
