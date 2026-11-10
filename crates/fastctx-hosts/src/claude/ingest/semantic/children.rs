//! Reverse traversal keeps ordinal keys and serializes one bounded cursor.
use crate::claude::ingest::document::{Address, Value, read::Reader};
use fastctx_types::sync::cursor::Count;
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Children {
    pub next: Option<Address>,
    pub remaining: Count,
}
impl Children {
    pub async fn of(reader: &Reader<'_>, array: Address) -> io::Result<Option<Self>> {
        Ok(match reader.node(array).await?.value {
            Value::Array { last, members } => Some(Self { next: last, remaining: members }),
            _ => None,
        })
    }
    pub async fn step(
        &mut self,
        reader: &Reader<'_>,
    ) -> io::Result<Option<(Address, crate::claude::ingest::document::Node)>> {
        let Some(address) = self.next else {
            if self.remaining.0 != 0 {
                return Err(io::Error::other("array ended before its declared count"));
            }
            return Ok(None);
        };
        let node = reader.node(address).await?;
        if self.remaining.0 == 0 || node.ordinal.0 != self.remaining.0 - 1 || node.name.is_some() {
            return Err(io::Error::other("array link has an invalid ordinal"));
        }
        self.remaining.0 -= 1;
        self.next = node.previous;
        Ok(Some((address, node)))
    }
}
