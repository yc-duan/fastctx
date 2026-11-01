//! A mail write resolves the member's current spelling in the same database
//! transaction. Catalog renames and member actors can therefore commit in
//! either order without letting a reused name acquire another member's count.

use super::Result;
use crate::store::{
    Letters,
    shelf::{self, Shelf},
};
use fastctx_net::sign::PublicKey;
use fastctx_storage::{Error, rusqlite::OptionalExtension};
use fastctx_types::MachineName;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) async fn persist(
    shelf: &Shelf,
    key: PublicKey,
    previous: MachineName,
    record: Letters,
    retired: Arc<AtomicBool>,
) -> Result<MachineName> {
    let member_key = serde_json::to_string(&key).map_err(|e| e.to_string())?;
    let bytes = crate::store::rows::body(&record).map_err(|e| e.to_string())?;
    shelf
        .0
        .write(bytes.len(), move |connection| {
            if retired.load(Ordering::Acquire) {
                return Err(Error::new("member was removed"));
            }
            let member: Option<Vec<u8>> = connection
                .query_row("SELECT body FROM hub_rows WHERE kind='members' AND key=?1", [&member_key], |row| row.get(0))
                .optional()?;
            let member = member.ok_or_else(|| Error::new("member was removed"))?;
            let current = shelf::decode::<fastctx_net::directory::Member>(&member)?.name;
            if previous.as_str() != current.as_str() {
                let old: Option<Vec<u8>> = connection
                    .query_row(
                        "SELECT body FROM hub_rows WHERE kind='letters' AND key=?1",
                        [previous.as_str()],
                        |row| row.get(0),
                    )
                    .optional()?;
                if old
                    .as_deref()
                    .map(shelf::decode::<Letters>)
                    .transpose()?
                    .is_some_and(|old| old.out_epoch == record.out_epoch)
                {
                    connection.execute("DELETE FROM hub_rows WHERE kind='letters' AND key=?1", [previous.as_str()])?;
                }
            }
            shelf::put(connection, "letters", current.as_str(), 0, &bytes)?;
            Ok(current)
        })
        .await
        .map_err(|e| e.to_string())
}
