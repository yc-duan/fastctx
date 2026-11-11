//! Timer pages limit discovery before filtering hidden or moving conversations.

use crate::records;
use fastctx_storage::Result;
use fastctx_types::timers::{Action, Owner, Timer};

pub(super) const ROWS: usize = 128;
pub(super) const ROW_BYTES: usize = 1024 * 1024;
const PAGE_BYTES: usize = 1024 * 1024;
const PAYLOAD_BYTES: usize = PAGE_BYTES - 32 * 1024;

pub(super) fn read(
    rows: &records::Rows<'_>,
    owner: Option<&Owner>,
    after: &str,
) -> Result<(Vec<Timer>, Option<String>)> {
    let category =
        owner.map(serde_json::to_string).transpose().map_err(|error| fastctx_storage::Error::new(error.to_string()))?;
    let sql = if category.is_some() {
        "SELECT CASE WHEN length(CAST(identity AS BLOB))<=4096 THEN identity END,CASE WHEN length(body)<=?3 THEN body END,EXISTS(SELECT 1 FROM node_conversation_transfers t WHERE t.name=r.conversation AND t.rejected=0) FROM conversation_records r INDEXED BY conversation_timers_visible_owner WHERE family='timers' AND json_extract(body,'$.action.kind') IS NOT 'sentinel' AND category=?1 AND identity>?2 ORDER BY identity LIMIT 129"
    } else {
        "SELECT CASE WHEN length(CAST(identity AS BLOB))<=4096 THEN identity END,CASE WHEN length(body)<=?3 THEN body END,EXISTS(SELECT 1 FROM node_conversation_transfers t WHERE t.name=r.conversation AND t.rejected=0) FROM conversation_records r INDEXED BY conversation_timers_visible WHERE family='timers' AND json_extract(body,'$.action.kind') IS NOT 'sentinel' AND identity>?2 ORDER BY identity LIMIT 129"
    };
    let mut query = rows.connection().prepare_cached(sql)?;
    let mut found = query.query((category, after, ROW_BYTES as i64))?;
    let mut timers = Vec::new();
    let mut bytes = 0;
    let mut reply_bytes = 2;
    let mut last = after.to_string();
    let mut next = None;
    for count in 0..=ROWS {
        let Some(row) = found.next()? else { break };
        if count == ROWS {
            next = Some(last);
            break;
        }
        let id: Option<String> = row.get(0)?;
        let id = id.ok_or_else(|| fastctx_storage::Error::new("a stored timer identity exceeds 4096 bytes"))?;
        let body: Option<Vec<u8>> = row.get(1)?;
        let moving: bool = row.get(2)?;
        if moving {
            last = id;
            continue;
        }
        let body =
            body.ok_or_else(|| fastctx_storage::Error::new("a stored timer exceeds the 1048576 byte read limit"))?;
        if bytes + body.len() > PAGE_BYTES {
            next = Some(last);
            break;
        }
        bytes += body.len();
        let timer: Timer = records::decode(&body)?;
        if !matches!(timer.action, Action::Sentinel { .. }) {
            let size =
                serde_json::to_vec(&timer).map_err(|error| fastctx_storage::Error::new(error.to_string()))?.len() + 1;
            if size + 2 > PAYLOAD_BYTES {
                return Err(fastctx_storage::Error::new(
                    "a stored timer exceeds the 1015808 byte page payload limit; it can still be cancelled",
                ));
            }
            if reply_bytes + size > PAYLOAD_BYTES {
                next = Some(last);
                break;
            }
            reply_bytes += size;
            timers.push(timer);
        }
        last = id;
    }
    Ok((timers, next))
}
