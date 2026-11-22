//! A library's database written out apart from its large objects, to go
//! where its hub moves ahead of them.
//!
//! The bytes of a large object are a file beside the database, and a
//! library's are most of what a hub holds: sent before the library opens
//! where the hub moved, every owner waits for them. So the copy of the
//! database that goes holds no large object at all, and says so: a library
//! whose rows said it held bytes that had not come would give out objects it
//! cannot read, and ask no device for them. Each that something still refers
//! to follows and is put again (`Library::put`), a new life of its hash, as
//! one a device sends is; what nothing refers to stays behind, as collected.

use super::objects::hex;
use super::{BYTES_FILE, FILE, OBJECTS_DIRECTORY, ROW_BYTES};
use fastctx_storage::rusqlite::Connection;
use std::path::{Path, PathBuf};

/// What refers to an object or holds it, each with what its rows must also
/// be: with none of these an object is collected (`Library::collect`).
const HOLDERS: [(&str, &str); 5] = [
    ("frame_objects", ""),
    ("parked_objects", ""),
    ("refused_objects", ""),
    ("object_refs", " AND t.count > 0"),
    ("object_holds", ""),
];

/// Whether the file named `name`, in a library's directory, is the database
/// whose rows say which objects the library holds.
pub fn names_database(name: &str) -> bool {
    name == FILE
}

/// Whether the library at `library` has both its databases where they are
/// opened: one under repair, a database of it set aside, has not, and is
/// not sent as the library it is not.
pub fn whole(library: &Path) -> bool {
    library.join(FILE).is_file() && library.join(BYTES_FILE).is_file()
}

/// Where that database is in the library at `library`.
pub fn database_in(library: &Path) -> PathBuf {
    library.join(FILE)
}

/// Whether the database `db` has the table `name`: a library no hub opened
/// since an earlier version has the tables that version made.
fn has(db: &Connection, name: &str) -> fastctx_storage::Result<bool> {
    Ok(db.query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)", [name], |row| row.get(0))?)
}

/// The large objects present in the library at `library`, whose database
/// `db` is, that a frame, a record or a hold refers to: each by its hash and
/// length, with the file its bytes are in.
pub fn large_held(db: &Connection, library: &Path) -> fastctx_storage::Result<Vec<([u8; 32], u64, PathBuf)>> {
    if !has(db, "objects")? {
        return Ok(Vec::new());
    }
    let mut held = Vec::new();
    for (table, more) in HOLDERS {
        if has(db, table)? {
            held.push(format!("EXISTS (SELECT 1 FROM {table} t WHERE t.hash = o.hash{more})"));
        }
    }
    if held.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = db.prepare(&format!(
        "SELECT o.hash, o.life, o.size FROM objects o WHERE o.state = 1 AND o.size > ?1 AND ({}) ORDER BY o.hash, o.life",
        held.join(" OR ")
    ))?;
    let rows = statement.query_map([ROW_BYTES as i64], |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)))?;
    let objects = library.join(OBJECTS_DIRECTORY);
    let mut found = Vec::new();
    for row in rows {
        let (hash, life, size) = row?;
        let Ok(hash) = <[u8; 32]>::try_from(hash.as_slice()) else { continue };
        let name = hex(&hash);
        found.push((hash, size.max(0) as u64, objects.join(&name[..2]).join(format!("{name}.{life}"))));
    }
    Ok(found)
}

/// Makes the copy `db` of a library's database hold no large object, their
/// bytes not being with it: the rows of those present go as a deleted life's
/// do (`Library::delete_life`), and those of tombstones, which have no bytes
/// left to delete there. In the transaction `db` is in. How many were
/// present.
pub fn forgo_large(db: &Connection) -> fastctx_storage::Result<u64> {
    if !has(db, "objects")? {
        return Ok(0);
    }
    let large = ROW_BYTES as i64;
    let present: i64 = db.query_row("SELECT COUNT(*) FROM objects WHERE state = 1 AND size > ?1", [large], |row| row.get(0))?;
    if has(db, "published")? {
        db.execute(
            "DELETE FROM published WHERE EXISTS (SELECT 1 FROM objects o
             WHERE o.hash = published.hash AND o.life = published.life AND o.size > ?1 AND o.state IN (1, 2))",
            [large],
        )?;
    }
    db.execute("DELETE FROM objects WHERE size > ?1 AND state IN (1, 2)", [large])?;
    if has(db, "object_chunks")? {
        db.execute("DELETE FROM object_chunks WHERE NOT EXISTS (SELECT 1 FROM objects o WHERE o.hash = object_chunks.hash)", [])?;
    }
    Ok(present.max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::Library;
    use std::time::Duration;

    /// A library with a large object a hold refers to, one nothing refers
    /// to, and a small one: the first alone follows, and a library that
    /// forwent both takes it again as a device's.
    #[test]
    fn a_copy_holds_no_large_object_and_those_held_follow() {
        let dir = std::env::temp_dir().join(format!("v2-apart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory");
        let (library, _) = Library::open(&dir).expect("a library");
        let held = vec![7u8; ROW_BYTES + 1];
        let loose = vec![8u8; ROW_BYTES + 2];
        let small = vec![9u8; 16];
        let lease = library.put(&mut held.as_slice(), "test", Duration::from_secs(60)).expect("put");
        library.put(&mut loose.as_slice(), "test", Duration::from_secs(60)).expect("put");
        let kept = library.put(&mut small.as_slice(), "test", Duration::from_secs(60)).expect("put");
        let hash = lease.object.hash;
        library
            .database()
            .write_now(64, move |db| {
                db.execute("INSERT INTO object_holds (kind, holder, hash) VALUES ('evidence', 'test', ?1)", [&hash[..]])?;
                Ok(())
            })
            .expect("a hold");
        let at = dir.clone();
        let following = library.database().read_now(move |db| large_held(db, &at)).expect("those held");
        assert_eq!(following.len(), 1);
        assert_eq!((following[0].0, following[0].1), (lease.object.hash, held.len() as u64));
        assert_eq!(std::fs::read(&following[0].2).expect("its file"), held);
        assert_eq!(library.database().write_now(64, forgo_large).expect("forgone"), 2);
        let at = dir.clone();
        assert!(library.database().read_now(move |db| large_held(db, &at)).expect("those held").is_empty());
        drop(library);

        let (library, _) = Library::open(&dir).expect("the library again");
        assert_eq!(library.size(&lease.object.hash).expect("size"), None);
        assert_eq!(library.size(&kept.object.hash).expect("size"), Some(small.len() as u64));
        let again = library.put(&mut held.as_slice(), "test", Duration::from_secs(60)).expect("put again");
        assert_eq!(again.object, lease.object);
        assert_eq!(library.read(&lease.object.hash, 0, 16).expect("read"), held[..16].to_vec());
        drop(library);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
