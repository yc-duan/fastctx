//! Terminal answer debts reference an immutable outcome. Paying one reader
//! hydrates one payload; adding a reader never copies every preceding answer.
use super::{Owing, storage};
use crate::{delivery::terminals, records};
use fastctx_storage::{Result, rusqlite::params};
use fastctx_types::{
    Address, NoticeId,
    layout::ConversationDir,
    residents::{Delivery, Notice},
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum Stored {
    Terminal { terminal: Reference },
    Inline(Box<Owing>),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Reference {
    pub to: Address,
    pub id: NoticeId,
    pub at: Timestamp,
    pub colleague: Address,
    pub label: String,
    pub turn: String,
}
impl Stored {
    fn resolve(self, rows: &records::Rows<'_>) -> Result<Owing> {
        match self {
            Self::Inline(owing) => Ok(*owing),
            Self::Terminal { terminal: reference } => {
                let header = terminals::header(rows, &reference.turn)?.ok_or_else(|| {
                    fastctx_storage::Error::with_kind(
                        fastctx_storage::ErrorKind::Corrupt,
                        "answer debt has lost its terminal",
                    )
                })?;
                Ok(Owing {
                    to: reference.to,
                    delivery: Delivery {
                        id: reference.id,
                        at: reference.at,
                        notice: Notice::Answer {
                            colleague: reference.colleague,
                            label: reference.label,
                            turn: reference.turn,
                            end: terminals::end(rows, &header)?,
                        },
                    },
                })
            }
        }
    }
}

pub(crate) fn owe(rows: &records::Rows<'_>, reference: Reference) -> Result<()> {
    let key = reference.id.clone();
    let at = reference.at;
    if !storage::paid(rows, key.as_str())? && !rows.exists("outgoing-answers", key.as_str())? {
        if !rows.exists(terminals::FAMILY, &reference.turn)? {
            return Err(fastctx_storage::Error::new("answer debt requires its terminal"));
        }
        rows.put_ordered("outgoing-answers", key.as_str(), &Stored::Terminal { terminal: reference }, at)?;
    }
    Ok(())
}
pub(crate) fn get(rows: &records::Rows<'_>, key: &str) -> Result<Option<Owing>> {
    rows.get::<Stored>("outgoing-answers", key)?.map(|stored| stored.resolve(rows)).transpose()
}
pub(crate) fn all(directory: &ConversationDir) -> io::Result<Vec<Owing>> {
    storage::read_rows(directory, |rows| {
        rows.unsettled::<Stored>("outgoing-answers", "paid-answers", "undeliverable-answers")?
            .into_iter()
            .map(|stored| stored.resolve(rows))
            .collect()
    })
}

#[derive(Clone, Default)]
pub(crate) struct Cursor {
    arrival: Vec<u8>,
    identity: String,
}
/// One complete answer per read bounds memory by one admitted message, including
/// its terminal payload. The caller performs network delivery outside the read.
pub(crate) fn next(directory: &ConversationDir, after: &Cursor) -> io::Result<Option<(Cursor, Owing)>> {
    storage::read_rows(directory, |rows| {
        let mut query=rows.connection().prepare_cached("SELECT r.arrival,r.identity,r.body FROM conversation_records r WHERE r.conversation=?1 AND r.family='outgoing-answers' AND (r.arrival,r.identity)>(?2,?3) AND NOT EXISTS(SELECT 1 FROM conversation_records d WHERE d.conversation=r.conversation AND d.identity=r.identity AND d.family IN ('paid-answers','undeliverable-answers')) ORDER BY r.arrival,r.identity LIMIT 1")?;
        let mut found = query.query(params![rows.conversation(), after.arrival, after.identity])?;
        let Some(row) = found.next()? else { return Ok(None) };
        let cursor = Cursor { arrival: row.get(0)?, identity: row.get(1)? };
        let stored: Stored = records::decode(&row.get::<_, Vec<u8>>(2)?)?;
        Ok(Some((cursor, stored.resolve(rows)?)))
    })
}

/// A pending reference remains a debt even if its payload cannot currently be read.
pub(crate) fn has_pending(directory: &ConversationDir) -> io::Result<bool> {
    storage::read_rows(directory, |rows| {
        Ok(rows.connection().query_row(
        "SELECT EXISTS(SELECT 1 FROM conversation_records r WHERE r.conversation=?1 AND r.family='outgoing-answers' AND NOT EXISTS(SELECT 1 FROM conversation_records d WHERE d.conversation=r.conversation AND d.identity=r.identity AND d.family IN ('paid-answers','undeliverable-answers')))",
        [rows.conversation()], |row| row.get(0),
    )?)
    })
}
