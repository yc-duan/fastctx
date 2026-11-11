//! Small derived headers keep timer reads independent of legacy payload size.
//! Triggers update them with the authoritative row. Expression indexes on the
//! large payload would make DELETE reparse that payload (2026-11-11).

use super::store::Store;
use crate::records;
use fastctx_storage::{Error, Priority, Result, rusqlite::Row};
use fastctx_types::{
    NoticeId, ResidentId,
    timers::{Action, Owner, Timer},
};
use jiff::Timestamp;
use std::{io, sync::Arc};

use fastctx_sync::registry::derived::{Object, ObjectKind};

const OBJECTS: &[Object] = &[
    Object { kind: ObjectKind::Table, name: "conversation_timer_headers", sql: "CREATE TABLE IF NOT EXISTS conversation_timer_headers (
    conversation TEXT NOT NULL, position INTEGER NOT NULL,
    id TEXT, owner TEXT, route TEXT, resident TEXT, delivery TEXT,
    identity_bytes INTEGER NOT NULL, body_bytes INTEGER NOT NULL, identity_preview TEXT NOT NULL,
    PRIMARY KEY(conversation,position)
) WITHOUT ROWID;" },
    Object { kind: ObjectKind::Index, name: "conversation_timer_headers_delivery", sql: "CREATE INDEX IF NOT EXISTS conversation_timer_headers_delivery ON conversation_timer_headers(delivery);" },
    Object { kind: ObjectKind::Index, name: "conversation_timer_headers_visible", sql: "CREATE INDEX IF NOT EXISTS conversation_timer_headers_visible ON conversation_timer_headers(conversation,position,delivery,identity_bytes,body_bytes,identity_preview) WHERE route IS NOT 'sentinel';" },
    Object { kind: ObjectKind::Index, name: "conversation_timer_headers_owner", sql: "CREATE INDEX IF NOT EXISTS conversation_timer_headers_owner ON conversation_timer_headers(owner,conversation,position,delivery,identity_bytes,body_bytes,identity_preview) WHERE route IS NOT 'sentinel';" },
    Object { kind: ObjectKind::Trigger, name: "conversation_timer_header_insert", sql: "CREATE TRIGGER IF NOT EXISTS conversation_timer_header_insert AFTER INSERT ON conversation_records WHEN NEW.family='timers' BEGIN
    INSERT OR REPLACE INTO conversation_timer_headers(conversation,position,id,owner,route,resident,delivery,identity_bytes,body_bytes,identity_preview) VALUES(NEW.conversation,NEW.position,CASE WHEN length(CAST(NEW.identity AS BLOB))<=4096 THEN NEW.identity END,CASE WHEN length(CAST(json_extract(NEW.body,'$.owner') AS BLOB))<=65536 THEN json_extract(NEW.body,'$.owner') END,substr(json_extract(NEW.body,'$.action.kind'),1,32),CASE WHEN length(CAST(json_extract(NEW.body,'$.action.resident') AS BLOB))<=65536 THEN json_extract(NEW.body,'$.action.resident') END,json_extract(NEW.body,'$.delivery'),length(CAST(NEW.identity AS BLOB)),length(NEW.body),substr(NEW.identity,1,128));
END;" },
    Object { kind: ObjectKind::Trigger, name: "conversation_timer_header_update", sql: "CREATE TRIGGER IF NOT EXISTS conversation_timer_header_update AFTER UPDATE OF body,identity,family,conversation,position ON conversation_records WHEN OLD.family='timers' OR NEW.family='timers' BEGIN
    DELETE FROM conversation_timer_headers WHERE conversation=OLD.conversation AND position=OLD.position;
    INSERT OR REPLACE INTO conversation_timer_headers(conversation,position,id,owner,route,resident,delivery,identity_bytes,body_bytes,identity_preview) SELECT NEW.conversation,NEW.position,CASE WHEN length(CAST(NEW.identity AS BLOB))<=4096 THEN NEW.identity END,CASE WHEN length(CAST(json_extract(NEW.body,'$.owner') AS BLOB))<=65536 THEN json_extract(NEW.body,'$.owner') END,substr(json_extract(NEW.body,'$.action.kind'),1,32),CASE WHEN length(CAST(json_extract(NEW.body,'$.action.resident') AS BLOB))<=65536 THEN json_extract(NEW.body,'$.action.resident') END,json_extract(NEW.body,'$.delivery'),length(CAST(NEW.identity AS BLOB)),length(NEW.body),substr(NEW.identity,1,128) WHERE NEW.family='timers';
END;" },
    Object { kind: ObjectKind::Trigger, name: "conversation_timer_header_delete", sql: "CREATE TRIGGER IF NOT EXISTS conversation_timer_header_delete AFTER DELETE ON conversation_records WHEN OLD.family='timers' BEGIN
    DELETE FROM conversation_timer_headers WHERE conversation=OLD.conversation AND position=OLD.position;
END;" },
];

const REBUILD: &str = "INSERT OR REPLACE INTO conversation_timer_headers(conversation,position,id,owner,route,resident,delivery,identity_bytes,body_bytes,identity_preview) SELECT conversation,position,CASE WHEN length(CAST(identity AS BLOB))<=4096 THEN identity END,CASE WHEN length(CAST(json_extract(body,'$.owner') AS BLOB))<=65536 THEN json_extract(body,'$.owner') END,substr(json_extract(body,'$.action.kind'),1,32),CASE WHEN length(CAST(json_extract(body,'$.action.resident') AS BLOB))<=65536 THEN json_extract(body,'$.action.resident') END,json_extract(body,'$.delivery'),length(CAST(identity AS BLOB)),length(body),substr(identity,1,128) FROM conversation_records WHERE family='timers';";

pub(crate) fn ensure(db: &fastctx_storage::rusqlite::Connection) -> Result<()> {
    db.execute_batch("DROP INDEX IF EXISTS conversation_timers_visible; DROP INDEX IF EXISTS conversation_timers_visible_owner; DROP INDEX IF EXISTS conversation_timers_delivery; DROP INDEX IF EXISTS conversation_timers_header; DROP INDEX IF EXISTS conversation_timers_page; DROP INDEX IF EXISTS conversation_timers_owner_page;")?;
    fastctx_sync::registry::derived::ensure(db, "conversation_timer_headers", OBJECTS, &[], REBUILD)?;
    Ok(())
}

pub(super) enum Routing {
    Tell(ResidentId),
    Goal,
    Sentinel,
}

pub(super) struct Header {
    pub id: Option<String>,
    pub owner: Option<Owner>,
    pub routing: Routing,
    pub delivery: NoticeId,
    conversation: String,
    position: i64,
}

pub(super) fn get(rows: &records::Rows<'_>, value: &str, handle: bool) -> Result<Option<Header>> {
    let lookup = if handle {
        "SELECT conversation,position FROM conversation_timer_headers INDEXED BY conversation_timer_headers_delivery WHERE delivery=?1 LIMIT 2"
    } else {
        "SELECT conversation,position FROM conversation_records INDEXED BY conversation_timer_id WHERE family='timers' AND identity=?1 LIMIT 2"
    };
    let mut lookup = rows.connection().prepare_cached(lookup)?;
    let mut locations = lookup.query([value])?;
    let Some(location) = locations.next()? else { return Ok(None) };
    let conversation: String = location.get(0)?;
    let position: i64 = location.get(1)?;
    if locations.next()?.is_some() {
        return Err(Error::new("the timer handle is ambiguous; use its identity"));
    }
    let sql = "SELECT id,owner,route,resident,delivery,conversation,position FROM conversation_timer_headers r WHERE conversation=?1 AND position=?2 AND NOT EXISTS(SELECT 1 FROM node_conversation_transfers t WHERE t.name=r.conversation AND t.rejected=0)";
    let mut query = rows.connection().prepare_cached(sql)?;
    let mut found = query.query((&conversation, position))?;
    found.next()?.map(decode).transpose()
}

fn decode(row: &Row<'_>) -> Result<Header> {
    let kind: String = row.get(2)?;
    let routing = match kind.as_str() {
        "tell" => Routing::Tell(
            row.get::<_, Option<String>>(3)?
                .ok_or_else(|| Error::new("a stored timer conversation exceeds 65536 bytes"))?
                .parse()
                .map_err(|e: fastctx_types::NameError| Error::new(e.to_string()))?,
        ),
        "goal" => Routing::Goal,
        "sentinel" => Routing::Sentinel,
        _ => return Err(Error::new("invalid stored timer action")),
    };
    let owner = row
        .get::<_, Option<String>>(1)?
        .ok_or_else(|| Error::new("the stored timer owner is missing or exceeds 65536 bytes"))?;
    let owner = serde_json::from_str(&owner).map_err(|error| Error::new(error.to_string()))?;
    Ok(Header {
        id: row.get(0)?,
        owner: Some(owner),
        routing,
        delivery: row
            .get::<_, Option<String>>(4)?
            .ok_or_else(|| Error::new("the stored timer has no delivery handle"))?
            .parse()
            .map_err(|e: fastctx_types::NameError| Error::new(e.to_string()))?,
        conversation: row.get(5)?,
        position: row.get(6)?,
    })
}

impl Store {
    pub(super) async fn header(self: &Arc<Self>, value: &str, handle: bool) -> io::Result<Option<Header>> {
        let value = value.to_string();
        self.work(move |store| records::read(&store.directory, |rows| get(rows, &value, handle))).await
    }
    pub(super) async fn cancel_header(self: &Arc<Self>, header: Header) -> io::Result<bool> {
        self.work(move |store| {
            let delivery = header.delivery.clone();
            let removed = records::write_at(&store.directory, Priority::Control, 1024, move |rows| {
                let current = get(rows, header.delivery.as_str(), true)?;
                if current.is_none_or(|current| {
                    current.conversation != header.conversation || current.position != header.position
                }) {
                    return Ok(false);
                }
                let changed = rows.connection().execute(
                    "DELETE FROM conversation_records WHERE family='timers' AND conversation=?1 AND position=?2",
                    (&header.conversation, header.position),
                )? != 0;
                if changed {
                    rows.scoped(&header.conversation).changed("timers")?;
                }
                Ok(changed)
            })?;
            if removed {
                store.claims.lock().expect("timer claims").settled_handle(&delivery, None);
            }
            store.changed.notify_one();
            Ok(removed)
        })
        .await
    }
    pub(super) async fn replace_header(
        self: &Arc<Self>,
        header: Header,
        owner: Owner,
        due: Timestamp,
        action: Action,
        delivery: NoticeId,
    ) -> io::Result<Option<Timer>> {
        let timer = Timer {
            id: header.id.clone().unwrap_or_default(),
            created_at: Some(Timestamp::now()),
            owner,
            due,
            action,
            delivery,
            attempts: 0,
            next_attempt: due,
            fired_at: None,
            error: None,
        };
        let payload = serde_json::to_vec(&timer)?;
        self.work(move |store| {
            let old_delivery = header.delivery.clone();
            let new_delivery = timer.delivery.clone();
            let result = records::write_at(&store.directory, Priority::Interactive, payload.len()+1024, move |rows| {
                let current = get(rows,header.delivery.as_str(),true)?.ok_or_else(||Error::new("the timer changed; read its current handle before replacing it"))?;
                if current.conversation != header.conversation || current.position != header.position || current.owner.as_ref() != Some(&timer.owner) {
                    return Err(Error::new("cancel a timer before changing its owner"));
                }
                let order = (due.as_nanosecond() ^ i128::MIN).to_be_bytes();
                rows.connection().execute("UPDATE conversation_records SET body=CAST(json_set(CAST(?3 AS TEXT),'$.id',identity) AS BLOB),arrival=?4,modified=?5 WHERE family='timers' AND conversation=?1 AND position=?2", (&header.conversation,header.position,payload,order.as_slice(),Timestamp::now().to_string()))?;
                rows.scoped(&header.conversation).changed("timers")?;
                Ok(header.id.is_some().then_some(timer))
            })?;
            store.claims.lock().expect("timer claims").settled_handle(&old_delivery,Some(&new_delivery));
            store.changed.notify_one();
            Ok(result)
        }).await
    }
}
