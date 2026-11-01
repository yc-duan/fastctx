//! Each request owns one replaceable effort and usage row; closing seals the turn.
use crate::records::{self, Source};
use fastctx_storage::{Database, Error, ErrorKind, Priority, Result, rusqlite::params};
use fastctx_types::{layout::ConversationDir, session::RequestUsage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, io};

const CLOSED: &str = "claude-turn-facts-closed";
#[derive(Default, Deserialize)]
struct Legacy {
    efforts: BTreeMap<u64, Value>,
    responses: BTreeMap<u64, RequestUsage>,
}
#[derive(Serialize, Deserialize)]
struct Closed {
    turn: String,
}
fn key(turn: &str) -> String {
    crate::speech::fixed_id("turn", &[turn]).to_string()
}
fn family(turn: &str) -> String {
    format!("claude-turn-facts/{}", key(turn))
}

fn prepare(directory: &ConversationDir, turn: &str) -> io::Result<String> {
    let family = family(turn);
    records::ensure_each(directory, &family, |push| {
        let path = directory.turn_facts().join(format!("{}.json", key(turn)));
        let legacy: Legacy = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for (step, value) in legacy.efforts {
            push(Source::new(format!("effort-{step:020}"), path.clone(), serde_json::to_vec(&value)?)?)?;
        }
        for (step, value) in legacy.responses {
            push(Source::new(format!("response-{step:020}"), path.clone(), serde_json::to_vec(&value)?)?)?;
        }
        Ok(())
    })?;
    Ok(family)
}

pub(super) fn put(directory: &ConversationDir, turn: &str, kind: &str, step: u64, bytes: Vec<u8>) -> io::Result<()> {
    let family = prepare(directory, turn)?;
    crate::delivery::prepare(directory)?;
    let (turn, id, member) = (turn.to_string(), key(turn), format!("{kind}-{step:020}"));
    records::write_at(directory, Priority::Normal, bytes.len() + member.len() + 256, move |rows| {
        if rows.exists("terminals", &turn)? || rows.exists(CLOSED, &id)? {
            return Ok(());
        }
        rows.put_raw(&family, &member, &bytes)
    })
}

pub(super) fn read<T>(
    directory: &ConversationDir,
    turn: &str,
    action: impl FnOnce(&records::Rows<'_>, &str) -> Result<T>,
) -> io::Result<Option<T>> {
    let family = prepare(directory, turn)?;
    records::read_background(directory, |rows| {
        if rows.exists(CLOSED, &key(turn))? { Ok(None) } else { action(rows, &family).map(Some) }
    })
}

pub(super) fn close(directory: &ConversationDir, turn: &str) -> io::Result<()> {
    prepare(directory, turn)?;
    crate::delivery::prepare(directory)?;
    let (turn, id) = (turn.to_string(), key(turn));
    records::write_at(directory, Priority::Normal, turn.len() + 256, move |rows| {
        if !rows.exists("terminals", &turn)? {
            return Err(Error::with_kind(
                ErrorKind::Invalid,
                "request observations cannot retire before their terminal",
            ));
        }
        if rows.exists(CLOSED, &id)? {
            return Ok(());
        }
        rows.put(CLOSED, &id, &Closed { turn })?;
        rows.connection().execute(
            "UPDATE conversation_records SET category='pending' WHERE conversation=?1 AND family=?2 AND identity=?3",
            (rows.conversation(), CLOSED, id),
        )?;
        Ok(())
    })
}

/// The terminal is already durable. Request rows retire in small background
/// commits; the closed marker prevents late host observations from reviving them.
pub(crate) fn collect(database: &Database, mut report: impl FnMut(&str, &Error)) -> Result<()> {
    let mut after = (String::new(), String::new());
    loop {
        let heads=database.read_background_now(|db| {
            let mut query=db.prepare_cached("SELECT conversation,identity,body FROM conversation_records WHERE family=?1 AND category='pending' AND (conversation,identity)>(?2,?3) ORDER BY conversation,identity LIMIT 16")?;
            Ok(query.query_map((CLOSED,&after.0,&after.1),|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,Vec<u8>>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)
        })?;
        let Some(last) = heads.last() else {
            return Ok(());
        };
        after = (last.0.clone(), last.1.clone());
        for (scope, id, body) in heads {
            if let Err(error) = collect_one(database, &scope, &id, &body) {
                report(&scope, &error);
            }
        }
    }
}
fn collect_one(database: &Database, scope: &str, id: &str, body: &[u8]) -> Result<()> {
    let closed: Closed = records::decode(body)?;
    if key(&closed.turn) != id {
        return Err(Error::with_kind(ErrorKind::Corrupt, "closed request observations belong to another turn"));
    }
    let family = family(&closed.turn);
    loop {
        let members=database.read_background_now(|db| {
            let mut query=db.prepare_cached("SELECT identity,length(body) FROM conversation_records WHERE conversation=?1 AND family=?2 ORDER BY identity LIMIT 16")?;
            let mut members=Vec::new();let mut bytes=0;
            for row in query.query_map((scope,&family),|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?)))? {
                let (key,size)=row?;
                if !members.is_empty() && bytes+size as usize>128*1024 {break;}
                bytes+=size as usize;members.push(key);
            }
            Ok(members)
        })?;
        if members.is_empty() {
            break;
        }
        let (scope, family) = (scope.to_string(), family.clone());
        database.write_now_at(Priority::Background, 128 * 1024, move |db| {
            let mut delete = db.prepare_cached(
                "DELETE FROM conversation_records WHERE conversation=?1 AND family=?2 AND identity=?3",
            )?;
            for key in members {
                delete.execute((&scope, &family, key))?;
            }
            Ok(())
        })?;
    }
    let (scope, id) = (scope.to_string(), id.to_string());
    database.write_now_at(Priority::Background, 256, move |db| {
        db.execute(
            "UPDATE conversation_records SET category='retired' WHERE conversation=?1 AND family=?2 AND identity=?3",
            params![scope, CLOSED, id],
        )?;
        Ok(())
    })
}
