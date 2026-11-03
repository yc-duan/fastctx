//! Indexed state and ordered immutable input snapshots. State transitions never
//! decode or rewrite the input body; readers load only their selected handovers.
mod publication;
pub(super) use publication::put;

use super::{
    Handover,
    value::{Prepared, Value},
};
use crate::records::{self, Rows};
use fastctx_storage::{
    Error, Priority, Result,
    rusqlite::{OptionalExtension, params},
};
use fastctx_types::{NoticeId, layout::ConversationDir, residents::Pending, session::TurnEnd};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::{io, sync::LazyLock};

/// A head for each handover of a conversation, kept as long as the
/// conversation is, and the inputs of its current revision only: older ones
/// are swept as it moves on (`collect_inputs`).
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS handover_heads(
 conversation TEXT NOT NULL,identity TEXT NOT NULL,revision TEXT NOT NULL,
 context BLOB NOT NULL,unresolved BLOB,turn TEXT,attempting INTEGER NOT NULL,
 active INTEGER NOT NULL,bound INTEGER NOT NULL DEFAULT 0,modified TEXT NOT NULL,modified_millis INTEGER NOT NULL,
 PRIMARY KEY(conversation,identity)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS handover_turn ON handover_heads(conversation,turn,identity) WHERE turn IS NOT NULL;
CREATE INDEX IF NOT EXISTS handover_active ON handover_heads(conversation,identity) WHERE active=1;
CREATE INDEX IF NOT EXISTS handover_uncertain ON handover_heads(conversation,modified_millis,identity) WHERE unresolved IS NOT NULL;
CREATE TABLE IF NOT EXISTS handover_inputs(
 conversation TEXT NOT NULL,handover TEXT NOT NULL,revision TEXT NOT NULL,
 ordinal INTEGER NOT NULL,identity TEXT NOT NULL,body BLOB NOT NULL,
 PRIMARY KEY(conversation,handover,revision,ordinal)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS handover_input ON handover_inputs(conversation,identity,handover,revision);
";
static GATES: LazyLock<crate::record_gates::Gates> = LazyLock::new(|| {
    crate::record_gates::Gates::new(fastctx_platform::doing::Rank::RECORDS, "publishing handover inputs")
});
#[derive(Serialize, Deserialize)]
struct Context {
    receipt: Option<crate::context::Receipt>,
    stamp: Option<crate::context::input::Stamp>,
}
pub(super) struct Header {
    revision: String,
    context: Value,
    unresolved: Option<Value>,
    turn: Option<String>,
    attempting: bool,
}
fn header(rows: &Rows<'_>, id: &str) -> Result<Option<Header>> {
    let data = rows.connection().query_row(
        "SELECT revision,context,unresolved,turn,attempting FROM handover_heads WHERE conversation=?1 AND identity=?2",
        (rows.conversation(), id), |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?, row.get::<_, Option<Vec<u8>>>(2)?, row.get::<_, Option<String>>(3)?, row.get::<_, bool>(4)?)),
    ).optional()?;
    data.map(|(revision, context, unresolved, turn, attempting)| {
        Ok(Header {
            revision,
            context: records::decode(&context)?,
            unresolved: unresolved.as_deref().map(records::decode).transpose()?,
            turn,
            attempting,
        })
    })
    .transpose()
}
fn project(rows: &Rows<'_>, id: NoticeId, header: Header) -> Result<Handover> {
    let mut statement = rows.connection().prepare_cached(
        "SELECT body FROM handover_inputs WHERE conversation=?1 AND handover=?2 AND revision=?3 ORDER BY ordinal",
    )?;
    let inputs = statement
        .query_map((rows.conversation(), id.as_str(), &header.revision), |row| row.get::<_, Vec<u8>>(0))?
        .map(|bytes| records::decode::<Value>(&bytes?)?.read(rows))
        .collect::<Result<Vec<Pending>>>()?;
    let context: Context = header.context.read(rows)?;
    Ok(Handover {
        id,
        inputs,
        context: context.receipt,
        context_input: context.stamp,
        turn: header.turn,
        unresolved: header.unresolved.map(|value| value.read(rows)).transpose()?,
        attempting: header.attempting,
        revision: header.revision,
    })
}
pub(super) fn get(rows: &Rows<'_>, id: &NoticeId) -> Result<Option<Handover>> {
    header(rows, id.as_str())?.map(|header| project(rows, id.clone(), header)).transpose()
}
/// A receipt-only lookup must not read all inputs sharing this handover.
pub(crate) fn stamp(directory: &ConversationDir, id: &NoticeId) -> io::Result<Option<crate::context::input::Stamp>> {
    super::storage::initialize(directory)?;
    records::read(directory, |rows| {
        header(rows, id.as_str())?
            .map(|header| header.context.read::<Context>(rows).map(|context| context.stamp))
            .transpose()
            .map(Option::flatten)
    })
}
fn root(id: &str, revision: &str, ordinal: usize) -> String {
    format!("{id}/{revision}/{ordinal}")
}

/// What one handover's record admits; a longer wait goes in several.
pub(crate) const INPUTS: usize = 16_384;
pub(crate) const BYTES: usize = 64 * 1024 * 1024;

pub(super) enum Change {
    Refused,
    Accepted(String),
    Unresolved(TurnEnd),
}
pub(super) fn update(directory: &ConversationDir, handover: &mut Handover, change: Change) -> io::Result<()> {
    super::storage::initialize(directory)?;
    crate::inbox::storage::initialize(directory)?;
    let end = match &change {
        Change::Unresolved(end) => Some(Prepared::new(directory, end)?),
        _ => None,
    };
    let id = handover.id.clone();
    let revised = records::write_at(directory, Priority::Interactive, 16 * 1024, move |rows| {
        let mut head = header(rows, id.as_str())?.ok_or_else(|| Error::new("handover is unavailable"))?;
        match change {
            Change::Refused => {
                if head.turn.is_none() && head.unresolved.is_none() {
                    rows.connection().execute("UPDATE conversation_records SET body=CAST(json_set(body,'$.state','waiting') AS BLOB) WHERE conversation=?1 AND family='pending-inputs' AND identity IN (SELECT identity FROM handover_inputs WHERE conversation=?1 AND handover=?2 AND revision=?3) AND json_extract(body,'$.state')='sending'", (rows.conversation(), id.as_str(), &head.revision))?;
                    rows.connection().execute("UPDATE handover_heads SET attempting=0,active=0,modified=?3,modified_millis=?4 WHERE conversation=?1 AND identity=?2", (rows.conversation(), id.as_str(), Timestamp::now().to_string(), Timestamp::now().as_millisecond()))?;
                    head.attempting = false;
                }
            }
            Change::Accepted(turn) => {
                if head.turn.as_ref().is_some_and(|prior| prior != &turn) {
                    return Err(Error::new("one handover cannot belong to two turns"));
                }
                rows.connection().execute(
                    "UPDATE handover_heads SET turn=?3,active=1,bound=0 WHERE conversation=?1 AND identity=?2",
                    (rows.conversation(), id.as_str(), &turn),
                )?;
                head.turn = Some(turn);
            }
            Change::Unresolved(_) => {
                if head.turn.is_some() {
                    return Err(Error::new("a handover with a known turn is not an unknown submission"));
                }
                if head.unresolved.is_none() {
                    let end = end.expect("prepared uncertainty");
                    end.install(rows, "handover-unresolved", id.as_str())?;
                    rows.connection().execute("UPDATE handover_heads SET unresolved=?3,active=1,modified=?4,modified_millis=?5 WHERE conversation=?1 AND identity=?2", params![rows.conversation(), id.as_str(), records::encode(&end.value)?, Timestamp::now().to_string(), Timestamp::now().as_millisecond()])?;
                    head.unresolved = Some(end.value);
                }
            }
        }
        Ok((head.revision, head.turn, head.attempting, head.unresolved))
    })?;
    if handover.revision != revised.0 {
        *handover =
            super::handover(directory, &handover.id)?.ok_or_else(|| io::Error::other("handover disappeared"))?;
    } else {
        handover.turn = revised.1;
        handover.attempting = revised.2;
        handover.unresolved = records::read(directory, |rows| revised.3.map(|value| value.read(rows)).transpose())?;
    }
    Ok(())
}

pub(crate) fn for_turn(directory: &ConversationDir, turn: &str) -> io::Result<Vec<Handover>> {
    select(directory, "turn=?2", Some(turn))
}
pub(crate) fn active(directory: &ConversationDir) -> io::Result<Vec<Handover>> {
    select(directory, "active=1", None)
}
pub(crate) fn uncertain(
    directory: &ConversationDir,
    since: Option<std::time::SystemTime>,
) -> io::Result<Vec<Handover>> {
    let since = since.map(Timestamp::try_from).transpose().map_err(io::Error::other)?;
    let since = since.map_or(i64::MIN, |at| at.as_millisecond()).to_string();
    select(directory, "unresolved IS NOT NULL AND modified_millis>=CAST(?2 AS INTEGER)", Some(&since))
}
fn select(directory: &ConversationDir, condition: &str, parameter: Option<&str>) -> io::Result<Vec<Handover>> {
    super::storage::initialize(directory)?;
    records::read(directory, |rows| {
        let sql = format!(
            "SELECT identity FROM handover_heads WHERE conversation=?1 AND {condition} AND (?2 IS NULL OR ?2 IS NOT NULL) ORDER BY identity"
        );
        let mut query = rows.connection().prepare(&sql)?;
        let ids = query
            .query_map((rows.conversation(), parameter), |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        // One unreadable handover is counted and left where it is; it does not
        // hold back the conversation's other handovers.
        let mut handovers = Vec::with_capacity(ids.len());
        for id in ids {
            let read = id
                .parse()
                .map_err(|error| {
                    Error::with_kind(fastctx_storage::ErrorKind::Corrupt, format!("invalid handover: {error}"))
                })
                .and_then(|id| get(rows, &id));
            match read {
                Ok(Some(handover)) => handovers.push(handover),
                Ok(None) => {}
                Err(error) if error.kind() == fastctx_storage::ErrorKind::Corrupt && !error.is_database_damage() => {
                    fastctx_platform::readings::counter("node.handovers.unreadable").add(1);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(handovers)
    })
}
/// Inputs are marked as being sent only once all of them are, and only then go
/// to the host: one of this handover's still waiting means none of them went.
pub(crate) fn unsent(directory: &ConversationDir, id: &NoticeId) -> io::Result<bool> {
    super::storage::initialize(directory)?;
    crate::inbox::storage::initialize(directory)?;
    records::read(directory, |rows| {
        let Some(head) = header(rows, id.as_str())? else { return Ok(false) };
        Ok(rows.connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM conversation_records WHERE conversation=?1 AND family='pending-inputs' AND identity IN (SELECT identity FROM handover_inputs WHERE conversation=?1 AND handover=?2 AND revision=?3) AND json_extract(body,'$.state')='waiting')",
            (rows.conversation(), id.as_str(), &head.revision),
            |row| row.get(0),
        )?)
    })
}

pub(crate) fn has_active(directory: &ConversationDir) -> io::Result<bool> {
    super::storage::initialize(directory)?;
    records::read(directory, |rows| {
        Ok(rows.connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM handover_heads WHERE conversation=?1 AND active=1)",
            [rows.conversation()],
            |row| row.get(0),
        )?)
    })
}
pub(super) fn input_state(rows: &Rows<'_>, id: &NoticeId) -> Result<(Option<String>, Option<String>, bool)> {
    let mut query = rows.connection().prepare_cached("SELECT h.turn,h.unresolved FROM handover_inputs i JOIN handover_heads h ON h.conversation=i.conversation AND h.identity=i.handover AND h.revision=i.revision WHERE i.conversation=?1 AND i.identity=?2 ORDER BY h.identity")?;
    let found = query.query_map((rows.conversation(), id.as_str()), |row| {
        Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<Vec<u8>>>(1)?))
    })?;
    let (mut turn, mut why, mut covered) = (None, None, false);
    for result in found {
        let (accepted, unresolved) = result?;
        covered = true;
        turn = turn.or(accepted);
        if why.is_none()
            && let Some(bytes) = unresolved
        {
            why = records::decode::<Value>(&bytes)?.read::<TurnEnd>(rows)?.reason;
        }
    }
    Ok((turn, why, covered))
}
pub(crate) fn accepted_inputs(
    directory: &ConversationDir,
    inputs: &[Pending],
    uncertainty: bool,
) -> io::Result<std::collections::HashSet<NoticeId>> {
    super::storage::initialize(directory)?;
    records::read(directory, |rows| {
        let mut known = std::collections::HashSet::new();
        for input in inputs {
            let accepted: bool = rows.connection().query_row("SELECT EXISTS(SELECT 1 FROM handover_inputs i JOIN handover_heads h ON h.conversation=i.conversation AND h.identity=i.handover AND h.revision=i.revision WHERE i.conversation=?1 AND i.identity=?2 AND (h.turn IS NOT NULL OR (?3 AND h.unresolved IS NOT NULL)))", (rows.conversation(), input.delivery.id.as_str(), uncertainty), |row| row.get(0))?;
            if accepted {
                known.insert(input.delivery.id.clone());
            }
        }
        Ok(known)
    })
}
pub(crate) fn complete(directory: &ConversationDir, id: &NoticeId) -> io::Result<()> {
    let id = id.to_string();
    records::write_at(directory, Priority::Normal, 128, move |rows| {
        rows.connection().execute(
            "UPDATE handover_heads SET active=0,bound=1 WHERE conversation=?1 AND identity=?2",
            (rows.conversation(), id),
        )?;
        Ok(())
    })
}
pub(crate) fn complete_turn(rows: &Rows<'_>, turn: &str) -> Result<()> {
    rows.connection().execute(
        "UPDATE handover_heads SET active=0 WHERE conversation=?1 AND turn=?2 AND bound=1",
        (rows.conversation(), turn),
    )?;
    Ok(())
}
pub(crate) fn modified(directory: &ConversationDir, id: &NoticeId) -> io::Result<Option<Timestamp>> {
    super::storage::initialize(directory)?;
    records::read(directory, |rows| {
        let at: Option<String> = rows
            .connection()
            .query_row(
                "SELECT modified FROM handover_heads WHERE conversation=?1 AND identity=?2",
                (rows.conversation(), id.as_str()),
                |row| row.get(0),
            )
            .optional()?;
        at.map(|at| at.parse().map_err(|error: jiff::Error| Error::new(error.to_string()))).transpose()
    })
}

// The caller owns this handover's publication gate. Delete at most sixteen
// obsolete inputs per transaction; a failed staging attempt cannot accumulate
// a new historical snapshot every time it is retried.
fn collect_inputs(directory: &ConversationDir, id: &str) -> io::Result<()> {
    loop {
        let id = id.to_string();
        let count = records::write_at(directory, Priority::Background, 16 * 8192, move |rows| {
            let mut query = rows.connection().prepare_cached("SELECT revision,ordinal FROM handover_inputs i WHERE conversation=?1 AND handover=?2 AND NOT EXISTS(SELECT 1 FROM handover_heads h WHERE h.conversation=i.conversation AND h.identity=i.handover AND h.revision=i.revision) LIMIT 16")?;
            let old = query
                .query_map((rows.conversation(), &id), |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for (revision, ordinal) in &old {
                crate::record_payloads::link(
                    rows.connection(),
                    rows.conversation(),
                    "handover-inputs",
                    &root(&id, revision, *ordinal as usize),
                    None,
                )?;
                rows.connection().execute(
                    "DELETE FROM handover_inputs WHERE conversation=?1 AND handover=?2 AND revision=?3 AND ordinal=?4",
                    (rows.conversation(), &id, revision, ordinal),
                )?;
            }
            Ok(old.len())
        })?;
        if count == 0 {
            return Ok(());
        }
    }
}

/// Binding finishes after the composer's receipt, the heard event and inbox
/// settlement. A completed terminal alone cannot prove those effects finished.
pub(crate) fn bound(directory: &ConversationDir, id: &NoticeId) -> io::Result<()> {
    let id = id.to_string();
    records::write_at(directory, Priority::Normal, 256, move |rows| {
        rows.connection().execute("UPDATE handover_heads SET bound=1,active=CASE WHEN EXISTS(SELECT 1 FROM conversation_records r WHERE r.conversation=handover_heads.conversation AND r.family='terminal-heads' AND r.identity=handover_heads.turn AND json_extract(r.body,'$.effects_complete')=1) THEN 0 ELSE 1 END WHERE conversation=?1 AND identity=?2", (rows.conversation(), id))?;
        Ok(())
    })
}
