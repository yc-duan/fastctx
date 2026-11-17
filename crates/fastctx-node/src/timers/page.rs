//! Timer discovery uses bounded metadata indexes before loading any payload.

use crate::records;
use fastctx_storage::{Error, Result};
use fastctx_types::timers::{OversizedTimer, Owner, Timer};
use serde::{Deserialize, Serialize};

pub(super) const ROWS: usize = 128;
const PAGE_BYTES: usize = 1024 * 1024;
const PAYLOAD_BYTES: usize = PAGE_BYTES - 128 * 1024;

#[derive(Default, Deserialize, Serialize)]
struct Cursor {
    conversation: String,
    position: i64,
}

pub(super) struct Page {
    pub timers: Vec<Timer>,
    pub oversized: Vec<OversizedTimer>,
    pub next: Option<String>,
}

pub(super) fn read(rows: &records::Rows<'_>, owner: Option<&Owner>, after: &str) -> Result<Page> {
    let cursor: Cursor = if after.is_empty() {
        Cursor::default()
    } else {
        serde_json::from_str(after).map_err(|error| Error::new(format!("invalid timer cursor: {error}")))?
    };
    let category = owner.map(serde_json::to_string).transpose().map_err(|error| Error::new(error.to_string()))?;
    let sql = if category.is_some() {
        "SELECT conversation,position,delivery,identity_bytes,body_bytes,identity_preview,EXISTS(SELECT 1 FROM node_conversation_transfers t WHERE t.name=r.conversation AND t.rejected=0) FROM conversation_timer_headers r INDEXED BY conversation_timer_headers_owner WHERE route IS NOT 'sentinel' AND owner=?1 AND (conversation,position)>(?2,?3) ORDER BY conversation,position LIMIT 129"
    } else {
        "SELECT conversation,position,delivery,identity_bytes,body_bytes,identity_preview,EXISTS(SELECT 1 FROM node_conversation_transfers t WHERE t.name=r.conversation AND t.rejected=0) FROM conversation_timer_headers r INDEXED BY conversation_timer_headers_visible WHERE route IS NOT 'sentinel' AND (conversation,position)>(?2,?3) ORDER BY conversation,position LIMIT 129"
    };
    let mut query = rows.connection().prepare_cached(sql)?;
    let mut bodies = rows.connection().prepare_cached(
        "SELECT body FROM conversation_records WHERE conversation=?1 AND family='timers' AND position=?2 AND length(body)<=?3",
    )?;
    let mut found = query.query((category, &cursor.conversation, cursor.position))?;
    let mut page = Page { timers: Vec::new(), oversized: Vec::new(), next: None };
    let mut source_bytes = 0;
    let mut reply_bytes = 2;
    let mut last = cursor;
    for count in 0..=ROWS {
        let Some(row) = found.next()? else { break };
        if count == ROWS {
            page.next = Some(encode(&last)?);
            break;
        }
        let current = Cursor { conversation: row.get(0)?, position: row.get(1)? };
        let moving: bool = row.get(6)?;
        if moving {
            last = current;
            continue;
        }
        let identity_bytes = row.get::<_, i64>(3)? as u64;
        let body_bytes = row.get::<_, i64>(4)? as usize;
        if identity_bytes > 4096 || body_bytes + 1024 > PAYLOAD_BYTES {
            let marker = OversizedTimer {
                handle: row
                    .get::<_, String>(2)?
                    .parse()
                    .map_err(|error: fastctx_types::NameError| Error::new(error.to_string()))?,
                identity_preview: row.get(5)?,
                identity_bytes,
                body_bytes: body_bytes as u64,
            };
            let size = encoded_size(&marker)? + 1;
            if reply_bytes + size > PAYLOAD_BYTES {
                page.next = Some(encode(&last)?);
                break;
            }
            reply_bytes += size;
            page.oversized.push(marker);
            last = current;
            continue;
        }
        if source_bytes + body_bytes > PAGE_BYTES {
            page.next = Some(encode(&last)?);
            break;
        }
        let body: Vec<u8> =
            bodies.query_row((&current.conversation, current.position, body_bytes as i64), |row| row.get(0))?;
        source_bytes += body.len();
        let timer: Timer = records::decode(&body)?;
        let size = encoded_size(&timer)? + 1;
        if reply_bytes + size > PAYLOAD_BYTES {
            page.next = Some(encode(&last)?);
            break;
        }
        reply_bytes += size;
        page.timers.push(timer);
        last = current;
    }
    Ok(page)
}

fn encode(cursor: &Cursor) -> Result<String> {
    serde_json::to_string(cursor).map_err(|error| Error::new(error.to_string()))
}

fn encoded_size(value: &impl Serialize) -> Result<usize> {
    struct Size(usize);
    impl std::io::Write for Size {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut size = Size(0);
    serde_json::to_writer(&mut size, value).map_err(|error| Error::new(error.to_string()))?;
    Ok(size.0)
}
