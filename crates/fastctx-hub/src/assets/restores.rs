//! How many times the hub was found put back from a backup and sealed for
//! its recovery (design §29, 2.2). Each time the count goes up by one, in the
//! transaction that keeps the seal, and what the hub's sources number from
//! then on starts above everything they could have numbered before: the
//! machine log's numbers and the account records' versions start at the
//! count times 2^32, and the hub's own log names its pieces under the count.
//! So nothing a device kept of the history the restore lost shares a name
//! with what comes after, and numbers still read in the order they were made.
//! The count is kept in the witness outside the directory as well, and goes
//! on from the greater of the two: a restore takes the directory's back, not
//! the witness's, so the same backup put back twice is counted twice. The
//! hub reopened numbers in a count of its own too, so a copy taken while it
//! was sealed, put back after, counts less than the witness and is counted
//! anew (codex-5c30d5e9 R01): numbers skip, never repeat.

use fastctx_storage::Database;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};

const NAME: &str = "hub.recoveries";

/// The seal the hub puts on as it starts when its setting says so: not a
/// recovery.
pub(crate) const AT_START: &str = "the hub seals them each time it starts, until the super administrator opens them";

fn read(c: &Connection) -> fastctx_storage::Result<u64> {
    let kept: Option<Vec<u8>> = c
        .prepare_cached("SELECT value FROM hub_meta WHERE name = ?1")?
        .query_row([NAME], |row| row.get(0))
        .optional()?;
    Ok(kept.and_then(|bytes| std::str::from_utf8(&bytes).ok().and_then(|text| text.parse().ok())).unwrap_or(0))
}

/// How many times the hub was sealed for a recovery.
pub(crate) fn count(control: &Database) -> std::io::Result<u64> {
    control.read_now(read).map_err(std::io::Error::other)
}

/// The least number the hub's sources hand out after `count` recoveries.
pub(crate) fn floor(count: u64) -> u64 {
    count << 32
}

/// Keeps the seal `why` for a recovery, in the transaction `c` is in. The
/// count goes up, from the directory's count or `witnessed`, the witness's,
/// whichever is more, as a recovery's seal is first kept — and as a seal is
/// found kept already on a directory that counts less than the witness: a
/// copy taken while sealed, put back after the hub reopened and numbered
/// on. A seal kept already on a directory the witness counts no more than
/// is this recovery's, kept across a restart, and is not counted again. The
/// count after.
pub(crate) fn seal_in(c: &Connection, why: &str, at: i64, witnessed: u64) -> fastctx_storage::Result<u64> {
    let was: Option<String> =
        c.prepare_cached("SELECT why FROM asset_seal WHERE id = 1")?.query_row([], |row| row.get(0)).optional()?;
    let mut count = read(c)?;
    if was.as_deref().is_none_or(|was| was == AT_START) || witnessed > count {
        count = count.max(witnessed) + 1;
        c.prepare_cached("INSERT OR REPLACE INTO hub_meta (name, value) VALUES (?1, ?2)")?
            .execute(params![NAME, count.to_string().into_bytes()])?;
    }
    c.prepare_cached("INSERT OR REPLACE INTO asset_seal (id, why, at) VALUES (1, ?1, ?2)")?
        .execute(params![why, at])?;
    Ok(count)
}

/// The hub reopened after a recovery, in the transaction `c` is in: what it
/// numbers from now on is in a count of its own, past the directory's and
/// `witnessed`, the witness's. The count after.
pub(crate) fn reopen_in(c: &Connection, witnessed: u64) -> fastctx_storage::Result<u64> {
    let count = read(c)?.max(witnessed) + 1;
    c.prepare_cached("INSERT OR REPLACE INTO hub_meta (name, value) VALUES (?1, ?2)")?
        .execute(params![NAME, count.to_string().into_bytes()])?;
    Ok(count)
}
