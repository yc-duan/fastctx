//! Retirement clears only rebuildable observations and settled retry diagnostics.
//! The lifecycle record has one current boolean per conversation, no history or
//! expiry; restore replaces it. Each cleanup transaction removes at most 16 rows.
//! Birth configuration, delivery proofs, history and payload roots remain owned
//! by their families until their own retention rule or conversation deletion.

use crate::records;
use fastctx_storage::Priority;
use fastctx_types::layout::ConversationDir;
use std::io;

const FAMILY: &str = "books-lifecycle";
const BATCH: usize = 16;

pub(super) fn prepare(directory: &ConversationDir) -> io::Result<()> {
    records::ensure_each(directory, FAMILY, |push| {
        let path = directory.path().join("books-retired");
        if fastctx_platform::disk::exists(&path)? {
            push(records::Source::new("current".into(), path, b"true".to_vec())?)?;
        }
        Ok(())
    })
}
pub(crate) fn retired(directory: &ConversationDir) -> io::Result<bool> {
    prepare(directory)?;
    records::read(directory, |rows| Ok(rows.get(FAMILY, "current")?.unwrap_or(false)))
}

pub(super) fn finished(directory: &ConversationDir) -> io::Result<bool> {
    Ok(retired(directory)?
        && fastctx_platform::disk::exists(directory.path().join("books-retired"))?
        && !fastctx_platform::disk::exists(directory.seen())?
        && !fastctx_platform::disk::exists(directory.outbox())?)
}

fn write<T: Send + 'static>(
    directory: &ConversationDir,
    priority: Priority,
    action: impl FnOnce(&records::Rows<'_>) -> fastctx_storage::Result<T> + Send + 'static,
) -> io::Result<T> {
    let (database, conversation) = records::locate(directory)?;
    database
        .write_now_settled_at(priority, 16 * 1024, move |connection| {
            action(&records::Rows::in_transaction(connection, &conversation))
        })
        .map_err(io::Error::other)
}

#[derive(PartialEq, Eq)]
pub(super) enum Progress {
    Waiting,
    More,
    Retired,
}

pub(super) fn prepare_cleanup(directory: &ConversationDir) -> io::Result<()> {
    prepare(directory)?;
    crate::residents::activity::prepare(directory, "seen")?;
    crate::inbox::outgoing::prepare(directory)
}

/// The caller owns lifecycle and speech for at most four cleanup transactions
/// and one final marker transaction.
/// Releasing them between steps lets a queued restore precede the next batch.
pub(super) fn retire(directory: &ConversationDir) -> io::Result<Progress> {
    let mut batches = 0;
    for family in ["seen", "answer-retries"] {
        loop {
            if batches == 4 {
                return Ok(Progress::More);
            }
            let deleted = write(directory, Priority::Background, move |rows| {
                if pending(rows)? {
                    return Ok(None);
                }
                let keys = {
                    let mut query = rows.connection().prepare_cached("SELECT identity FROM conversation_records WHERE conversation=?1 AND family=?2 ORDER BY identity LIMIT ?3")?;
                    query
                        .query_map((rows.conversation(), family, BATCH as i64), |row| row.get::<_, String>(0))?
                        .collect::<Result<Vec<_>, _>>()?
                };
                for key in &keys {
                    rows.delete(family, key)?;
                }
                Ok(Some(keys.len()))
            })?;
            let Some(deleted) = deleted else { return Ok(Progress::Waiting) };
            batches += 1;
            fastctx_platform::readings::counter("node.books.rows_retired").add(deleted as u64);
            if deleted < BATCH {
                break;
            }
        }
    }
    let marked = write(directory, Priority::Background, |rows| {
        if pending(rows)? {
            return Ok(false);
        }
        rows.put(FAMILY, "current", &true)?;
        Ok(true)
    })?;
    if !marked {
        return Ok(Progress::Waiting);
    }
    // Only these legacy sources were prepared above. Other books include
    // unmigrated proofs and birth settings, so they are not directory garbage.
    fastctx_platform::owned_directory::delete(&directory.seen())?;
    fastctx_platform::owned_directory::delete(&directory.outbox())?;
    fastctx_platform::publish(&directory.path().join("books-retired"), b"", fastctx_platform::Permissions::Private)?;
    fastctx_platform::readings::counter("node.books.retirements").add(1);
    Ok(Progress::Retired)
}

pub(crate) fn resume(directory: &ConversationDir) -> io::Result<()> {
    prepare(directory)?;
    // A failed reset leaves the resident archived. Successful restore cannot
    // leave the legacy host-cleanup marker treating new work as retired.
    match fastctx_platform::disk::remove_file(directory.path().join("books-retired")) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    write(directory, Priority::Control, |rows| rows.put(FAMILY, "current", &false))
}

fn pending(rows: &records::Rows<'_>) -> fastctx_storage::Result<bool> {
    let sql = "SELECT
        EXISTS(SELECT 1 FROM terminal_work WHERE conversation=?1)
        OR EXISTS(SELECT 1 FROM conversation_records WHERE conversation=?1 AND family IN ('remote-running','remote-whole-answers','input-stops'))
        OR EXISTS(SELECT 1 FROM conversation_records r WHERE conversation=?1 AND family='outgoing-answers' AND NOT EXISTS(SELECT 1 FROM conversation_records d WHERE d.conversation=r.conversation AND d.identity=r.identity AND d.family IN ('paid-answers','undeliverable-answers')))
        OR EXISTS(SELECT 1 FROM conversation_records r WHERE conversation=?1 AND family='pending-inputs' AND NOT EXISTS(SELECT 1 FROM conversation_records d WHERE d.conversation=r.conversation AND d.identity=r.identity AND d.family='delivery-receipts'))
        OR EXISTS(SELECT 1 FROM conversation_records WHERE conversation=?1 AND family IN ('items/books/received','items/books/received-controls','items/books/job-deliveries','items/books/on-deliveries','items/books/on-stops','items/books/run-stops','items/books/run-requests') AND identity GLOB '*.json')
        OR EXISTS(SELECT 1 FROM conversation_records WHERE conversation=?1 AND family >= 'host-reports/' AND family < 'host-reports0')";
    rows.connection().query_row(sql, [rows.conversation()], |row| row.get(0)).map_err(Into::into)
}
