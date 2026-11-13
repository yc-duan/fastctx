//! The hub's source instance (design §29, 2.2): a random number drawn as the
//! hub's asset side is first made and again as it is reopened after a
//! recovery. The hub-ordered streams whose sources number their records — the
//! machine log's audit, the hub's own log — carry it in each item, so a number
//! a restored source hands out again names a new item, never one a device kept
//! of the history the restore lost.

use fastctx_storage::Database;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

const NAME: &str = "assets.instance";

/// A number no instance before it is likely to have drawn.
fn drawn() -> u64 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut hasher = Sha256::new();
    hasher.update(b"fastctx-hub-instance/1");
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed).to_le_bytes());
    if let Ok(elapsed) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        hasher.update(elapsed.as_nanos().to_le_bytes());
    }
    let digest: [u8; 32] = hasher.finalize().into();
    u64::from_le_bytes(digest[..8].try_into().expect("eight bytes"))
}

fn parsed(value: Vec<u8>) -> fastctx_storage::Result<u64> {
    std::str::from_utf8(&value)
        .ok()
        .and_then(|text| u64::from_str_radix(text, 16).ok())
        .ok_or_else(|| fastctx_storage::Error::new("a malformed source instance"))
}

/// The instance the hub is in, drawn the first time it is asked.
pub(crate) fn current(control: &Database) -> std::io::Result<u64> {
    control
        .write_now(128, |c| {
            let kept: Option<Vec<u8>> =
                c.prepare_cached("SELECT value FROM hub_meta WHERE name = ?1")?.query_row([NAME], |row| row.get(0)).optional()?;
            if let Some(kept) = kept {
                return parsed(kept);
            }
            let instance = drawn();
            c.prepare_cached("INSERT INTO hub_meta (name, value) VALUES (?1, ?2)")?
                .execute(params![NAME, format!("{instance:016x}").into_bytes()])?;
            Ok(instance)
        })
        .map_err(std::io::Error::other)
}

/// Draws the next instance, in the transaction `c` is in: the hub reopened
/// after a recovery numbers what its sources hand out again as another's.
pub(crate) fn renew_in(c: &Connection) -> fastctx_storage::Result<u64> {
    let instance = drawn();
    c.prepare_cached("INSERT OR REPLACE INTO hub_meta (name, value) VALUES (?1, ?2)")?
        .execute(params![NAME, format!("{instance:016x}").into_bytes()])?;
    Ok(instance)
}

/// `id` of the instance `instance`, as an item.
pub(crate) fn item(instance: u64, id: u64) -> Vec<u8> {
    format!("{instance:016x}/{id:016x}").into_bytes()
}

/// The instance and number of an item written by `item`.
pub(crate) fn of_item(item: &[u8]) -> Option<(u64, u64)> {
    let text = std::str::from_utf8(item).ok()?;
    let (instance, id) = text.split_once('/')?;
    Some((u64::from_str_radix(instance, 16).ok()?, u64::from_str_radix(id, 16).ok()?))
}
