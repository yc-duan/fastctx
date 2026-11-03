//! A database that did not open at start for a cause that passes, a full
//! disk or a lock another process held, leaves the node answering without
//! it (`fastctx_storage::Database::open_or_unavailable`), and nothing opens
//! it in place: every call that needs it fails until the node starts again.
//! So the node opens it again every few seconds, as it does at start, and
//! once it opens, ends for its service to start the node with it. A damaged
//! database was set aside and salvaged at start; that is not waited out.

use std::sync::Arc;
use std::time::Duration;

use crate::Node;

/// How often a database that did not open is tried again.
const EVERY: Duration = Duration::from_secs(5);

pub(crate) fn start(node: &Arc<Node>) {
    let waiting =
        |database: &fastctx_storage::Database| database.fault().is_some_and(|fault| !fault.is_database_damage());
    let (records, book) = (waiting(&node.database), waiting(&node.book_database));
    if !records && !book {
        return;
    }
    let world = node.world.clone();
    fastctx_platform::task::spawn(async move {
        loop {
            tokio::time::sleep(EVERY).await;
            let trying = world.clone();
            let opens = fastctx_platform::task::blocking(move || {
                (!records || crate::record_database::open(&trying).fault().is_none())
                    && (!book || crate::activity_book::open(&trying).fault().is_none())
            })
            .await;
            if opens {
                crate::ending::with_reason(
                    1,
                    "storage that did not open at start opens now; exiting for the service to start the node with it"
                        .into(),
                );
            }
        }
    });
}
