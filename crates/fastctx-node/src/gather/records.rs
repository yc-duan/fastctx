//! Gathering preserves logical record identities and invalidates only completed
//! imports. The generation is published before moving any source into a room.
use super::*;
use crate::record_migration::scheduler;
use fastctx_storage::{Database, rusqlite::OptionalExtension};
use std::cell::RefCell;

pub(super) const GENERATION: &str = ".record-generation";
pub(super) const ORIGINS: &str = ".queue-origins.json";
pub(super) const REJECTIONS: &str = ".queue-rejections.json";
/// A row for each conversation whose records were gathered, written over
/// at each gathering and gone with the conversation.
pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS conversation_record_gathering (
    conversation TEXT PRIMARY KEY, generation TEXT NOT NULL, boundary INTEGER NOT NULL,
    complete INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;";
thread_local! {
    static TOUCHED: RefCell<BTreeSet<PathBuf>> = const { RefCell::new(BTreeSet::new()) };
    static WORLD: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

pub(super) fn begin(world: &WorldDir) {
    WORLD.with_borrow_mut(|current| *current = Some(world.path().to_path_buf()));
    TOUCHED.with_borrow_mut(BTreeSet::clear);
}
pub(super) fn moving(destination: &Path) -> io::Result<()> {
    let Some(room) = WORLD.with_borrow(|world| {
        let conversations = world.as_ref()?.join("conversations");
        let first = destination.strip_prefix(&conversations).ok()?.components().next()?;
        Some(conversations.join(first.as_os_str()))
    }) else {
        return Ok(());
    };
    if TOUCHED.with_borrow(|rooms| rooms.contains(&room)) {
        return Ok(());
    }
    ensure_private_dir(&room)?;
    publish(&room.join(GENERATION), crate::speech::new_id("gather").as_str().as_bytes(), Permissions::Private)?;
    TOUCHED.with_borrow_mut(|rooms| rooms.insert(room.to_path_buf()));
    Ok(())
}

pub(crate) use super::layout::Layout;

pub(super) fn reset(world: &Path, database: &Database, conversation: &str, generation: &str) -> io::Result<()> {
    let (scope, token) = (conversation.to_string(), generation.to_string());
    let complete = database.write_now_at(scheduler::priority(), 256, move |db| {
        if crate::conversation_store::deleted(db, &scope)? { return Ok(true); }
        let prior: Option<(String, bool)> = db.query_row("SELECT generation,complete FROM conversation_record_gathering WHERE conversation=?1", [&scope], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
        if let Some((prior, complete)) = prior && prior == token { return Ok(complete); }
        db.execute("INSERT INTO conversation_record_gathering(conversation,generation,boundary) VALUES(?1,?2,COALESCE((SELECT position FROM conversation_record_positions WHERE conversation=?1 AND stream='records'),0)) ON CONFLICT(conversation) DO UPDATE SET generation=excluded.generation,boundary=excluded.boundary,complete=0", (&scope,token))?;
        Ok(false)
    }).map_err(io::Error::other)?;
    if complete {
        return Ok(());
    }
    // This generation is not usable until all its durable completion marks reset.
    // The current gather job keeps running; other in-flight families are cancelled.
    scheduler::invalidate(world, conversation);
    for statement in [
        "DELETE FROM conversation_record_imports WHERE conversation=?1 AND family IN (SELECT family FROM conversation_record_imports WHERE conversation=?1 LIMIT 128)",
        "DELETE FROM session_sources WHERE conversation=?1 AND identity IN (SELECT identity FROM session_sources WHERE conversation=?1 AND finished=1 LIMIT 128)",
        "DELETE FROM migration_row_proofs WHERE conversation=?1 AND (family,path,ordinal) IN (SELECT p.family,p.path,p.ordinal FROM migration_row_proofs p JOIN migration_files f ON f.family=p.family AND f.path=p.path WHERE p.conversation=?1 AND f.verified=1 LIMIT 128)",
        "DELETE FROM migration_files WHERE conversation=?1 AND (family,path) IN (SELECT family,path FROM migration_files WHERE conversation=?1 AND verified=1 LIMIT 128)",
    ] {
        loop {
            scheduler::checkpoint()?;
            let scope = conversation.to_string();
            let count = database
                .write_now_at(scheduler::priority(), 16384, move |db| Ok(db.execute(statement, [scope])?))
                .map_err(io::Error::other)?;
            if count == 0 {
                break;
            }
        }
    }
    let scope = conversation.to_string();
    database.write_now_at(scheduler::priority(), 256, move |db| {
        db.execute("DELETE FROM session_migration_state WHERE conversation=?1 AND EXISTS(SELECT 1 FROM session_heads WHERE conversation=?1 AND imported=1)",[&scope])?;
        db.execute("UPDATE session_heads SET imported=0 WHERE conversation=?1",[&scope])?;
        db.execute("UPDATE run_imported SET checked=0 WHERE conversation=?1",[&scope])?;
        db.execute("DELETE FROM migration_born WHERE name=?1",[&scope])?;
        db.execute("UPDATE conversation_record_gathering SET complete=1 WHERE conversation=?1",[scope])?;
        Ok(())
    }).map_err(io::Error::other)
}
