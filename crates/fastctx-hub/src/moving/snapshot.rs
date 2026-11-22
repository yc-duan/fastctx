//! A move copies a closed copy of each database, never a live database and
//! its log observed at different instants. The frozen round makes them
//! again.
//!
//! The hub's own databases are copied page for page: the copy is made in
//! the moments the hub is stopped for. An owner library's are written out
//! anew once nothing writes them, and the one whose rows say which objects
//! the library holds is made to hold no large object: their bytes are no
//! part of what is sent first, and follow once every library is open where
//! the hub moved (`fastctx_content::v2::forgo_large`, `following`).
use fastctx_storage::rusqlite::{
    Connection, OpenFlags,
    backup::{Backup, StepResult},
};
use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Where the copies are made, under the hub's directory: of no listing.
const COPIES: &str = ".hub-handover";
/// Where the copy of the hub's own database was made before each had its
/// place under `COPIES`.
const FORMER: &str = ".hub-handover.sqlite3";

/// Where the copy of the database at `path` under `dir` is.
pub(super) fn path(dir: &Path, path: &str) -> PathBuf {
    let mut place = dir.join(COPIES);
    place.extend(path.split('/'));
    place
}

/// Removes the copies made under `dir`, all of them or those below `under`.
pub(super) fn clear(dir: &Path, under: Option<&str>) -> io::Result<()> {
    let place = under.map_or_else(|| dir.join(COPIES), |under| path(dir, under));
    for gone in [place, dir.join(FORMER)] {
        match fastctx_platform::disk::symlink_metadata(&gone) {
            Ok(found) if found.is_dir() => fastctx_platform::disk::remove_dir_all(&gone)?,
            Ok(_) => fastctx_platform::disk::remove_file(&gone)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn remove(file: &Path) -> io::Result<()> {
    match fastctx_platform::disk::remove_file(file) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Makes the closed copy of the database at `database` under `dir`: page
/// for page; or, `library`, an owner library's, as the head of this file
/// says.
pub(super) fn capture(dir: &Path, database: &str, library: bool) -> io::Result<()> {
    let snapshot = path(dir, database);
    if let Some(parent) = snapshot.parent() {
        fastctx_platform::disk::create_dir_all(parent)?;
    }
    remove(&snapshot)?;
    let mut from = dir.to_path_buf();
    from.extend(database.split('/'));
    let source = Connection::open_with_flags(&from, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(io::Error::other)?;
    source.busy_timeout(Duration::from_secs(5)).map_err(io::Error::other)?;
    if !library {
        return pages(&source, &snapshot);
    }
    let to = snapshot.to_str().ok_or_else(|| io::Error::other(format!("{} is not UTF-8", snapshot.display())))?;
    // One that cannot be written out anew — a page of it does not read as
    // what it should hold — goes page for page, as the hub had it.
    if source.execute("VACUUM INTO ?1", [to]).is_err() {
        remove(&snapshot)?;
        pages(&source, &snapshot)?;
    }
    drop(source);
    if database.rsplit('/').next().is_some_and(fastctx_content::v2::names_database) {
        let copy = Connection::open(&snapshot).map_err(io::Error::other)?;
        // One file, with no log beside it, before and after.
        copy.pragma_update(None, "journal_mode", "DELETE").map_err(io::Error::other)?;
        let transaction = copy.unchecked_transaction().map_err(io::Error::other)?;
        fastctx_content::v2::forgo_large(&transaction).map_err(io::Error::other)?;
        transaction.commit().map_err(io::Error::other)?;
        copy.close().map_err(|(_, error)| io::Error::other(error))?;
    }
    Ok(())
}

/// Copies the database `source` reads into `snapshot`, page for page.
fn pages(source: &Connection, snapshot: &Path) -> io::Result<()> {
    let transaction = source.unchecked_transaction().map_err(io::Error::other)?;
    transaction
        .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))
        .map_err(io::Error::other)?;
    let mut target = Connection::open(snapshot).map_err(io::Error::other)?;
    target.pragma_update(None, "synchronous", "FULL").map_err(io::Error::other)?;
    {
        let backup = Backup::new(&transaction, &mut target).map_err(io::Error::other)?;
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            match backup.step(256).map_err(io::Error::other)? {
                StepResult::Done => break,
                StepResult::More => {}
                StepResult::Busy | StepResult::Locked => std::thread::sleep(Duration::from_millis(5)),
                _ => return Err(io::Error::other("unknown SQLite backup outcome")),
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other("the hub database snapshot did not finish within five minutes"));
            }
        }
    }
    // Leave one independently readable file, including when the source's
    // journal mode was copied in its first page.
    target.pragma_update(None, "journal_mode", "DELETE").map_err(io::Error::other)?;
    target.close().map_err(|(_, e)| io::Error::other(e))?;
    Ok(())
}

/// A large object that follows its library: its hash, its length, and the
/// file its bytes are in.
pub(super) type Following = ([u8; 32], u64, PathBuf);

/// The large objects of the owner library at `library` under `dir` that
/// follow it, as its database says now that nothing writes it: those a
/// frame, a record or a hold refers to.
pub(super) fn following(dir: &Path, library: &str) -> io::Result<Vec<Following>> {
    let mut place = dir.to_path_buf();
    place.extend(library.split('/'));
    // Not there, which of its objects follow is not known: none is said to.
    let database = fastctx_content::v2::database_in(&place);
    let source = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(io::Error::other)?;
    source.busy_timeout(Duration::from_secs(5)).map_err(io::Error::other)?;
    fastctx_content::v2::large_held(&source, &place).map_err(io::Error::other)
}
