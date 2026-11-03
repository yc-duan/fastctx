//! The accepted local goal, stop hold and frozen input set share a SQL commit.
//! Host files are projections; the local record never embeds a growing ID list.
use super::lifecycle::Hold;
use crate::{
    record_payloads as payloads,
    records::{self, Rows},
};
use fastctx_storage::{Priority, Result, rusqlite::OptionalExtension};
use fastctx_types::{
    NoticeId,
    goals::{Status, View},
    layout::ConversationDir,
};
use serde::{Deserialize, Serialize};
use std::{io, path::Path};

pub(super) const LOCAL: &str = "goal-local";
pub(super) const HOLD: &str = "goal-hold";
const HELD: &str = "goal-frozen-inputs";
/// A row for each conversation whose goal projection is yet to be
/// written, gone once it is (`projected`) and with the conversation.
pub(crate) const SCHEMA: &str =
    "CREATE TABLE IF NOT EXISTS goal_projection_dirty(conversation TEXT PRIMARY KEY) WITHOUT ROWID;";
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct HostBridge {
    pub active: bool,
    pub paused: bool,
    pub held: Option<Vec<NoticeId>>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Held {
    Revision { revision: u64 },
    Legacy,
    Empty,
}
#[derive(Clone, Serialize, Deserialize)]
struct Bridge {
    active: bool,
    paused: bool,
    held: Option<Held>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Local {
    #[serde(flatten)]
    pub view: View,
    #[serde(default, rename = "_claude_goal", skip_serializing_if = "Option::is_none")]
    bridge: Option<Bridge>,
}
#[derive(Deserialize)]
struct Legacy {
    #[serde(flatten)]
    view: View,
    #[serde(default, rename = "_claude_goal")]
    bridge: Option<HostBridge>,
}
pub(super) struct PreparedBridge {
    bridge: Bridge,
    payload: Option<payloads::Staged>,
}
impl PreparedBridge {
    fn prepare(directory: &ConversationDir, bridge: HostBridge) -> io::Result<Self> {
        Self::prepare_at(directory, bridge, Priority::Interactive)
    }
    fn prepare_at(directory: &ConversationDir, bridge: HostBridge, priority: Priority) -> io::Result<Self> {
        let (db, scope) = records::locate(directory)?;
        let payload = bridge
            .held
            .map(|held| payloads::stage(&db, &scope, records::encode(&held)?, priority))
            .transpose()
            .map_err(io::Error::other)?;
        Ok(Self {
            bridge: Bridge {
                active: bridge.active,
                paused: bridge.paused,
                held: payload.as_ref().map(|_| Held::Legacy),
            },
            payload,
        })
    }
    fn install(&self, rows: &Rows<'_>) -> Result<()> {
        if let Some(payload) = &self.payload {
            payloads::link(rows.connection(), rows.conversation(), HELD, "current", Some(&payload.reference))?;
        }
        Ok(())
    }
    pub(super) fn finish(&self, rows: &Rows<'_>) -> Result<()> {
        if let Some(payload) = &self.payload {
            payloads::finish(rows.connection(), rows.conversation(), payload)?;
        }
        Ok(())
    }
}
pub(crate) fn prepare(directory: &ConversationDir) -> io::Result<()> {
    prepare_hold(directory)?;
    let source = directory.clone();
    records::ensure_file(directory, LOCAL, directory.books().join("goal-local.json"), move |path, push| {
        let directory = &source;
        let bytes = match fastctx_platform::disk::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let old: Legacy = serde_json::from_slice(&bytes)?;
        let expected =
            old.bridge.as_ref().and_then(|bridge| bridge.held.as_ref()).map(serde_json::to_vec).transpose()?;
        let bridge = old.bridge.as_ref().map(|bridge| Bridge {
            active: bridge.active,
            paused: bridge.paused,
            held: bridge.held.as_ref().map(|_| Held::Legacy),
        });
        if let Some(legacy) = old.bridge {
            if !crate::record_migration::files::verifying() {
                let prepared =
                    PreparedBridge::prepare_at(directory, legacy, crate::record_migration::scheduler::priority())?;
                records::write_at(directory, Priority::Background, 256, move |rows| {
                    if !rows.exists(LOCAL, "current")? {
                        prepared.install(rows)?;
                    }
                    prepared.finish(rows)
                })?;
            }
            let stored = records::read(directory, |rows| {
                payloads::read_root(rows.connection(), rows.conversation(), HELD, "current")
            })?;
            if stored != expected {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "legacy frozen goal inputs did not verify"));
            }
        }
        push(records::Source::new(
            "current".into(),
            path.to_path_buf(),
            records::legacy::raw::merge(&bytes, &Local { view: old.view, bridge })?,
        )?)
    })
}
pub(super) fn prepare_hold(directory: &ConversationDir) -> io::Result<()> {
    records::legacy::single::<Hold>(directory, HOLD, &directory.books().join("goal-hold.json"))
}
pub(super) fn fallback(directory: &ConversationDir, exchange: Option<&Path>) -> io::Result<Option<PreparedBridge>> {
    let needed = records::read(directory, |rows| {
        Ok(rows.get::<Local>(LOCAL, "current")?.is_none_or(|local| local.bridge.is_none()))
    })?;
    if !needed {
        return Ok(None);
    }
    let Some(exchange) = exchange else { return Ok(None) };
    fastctx_hosts::claude::control::read::<HostBridge>(&exchange.join("goal.json"))?
        .map(|bridge| PreparedBridge::prepare(directory, bridge))
        .transpose()
}
/// Whether `kept` is a later record of the same account's goal than `view`:
/// one the account changed after `view` was read.
pub(super) fn newer(kept: &View, view: &View) -> bool {
    kept.account == view.account && matches!((kept.version, view.version), (Some(kept), Some(view)) if kept > view)
}

pub(super) fn get(directory: &ConversationDir) -> io::Result<Option<View>> {
    prepare(directory)?;
    records::read(directory, |rows| Ok(rows.get::<Local>(LOCAL, "current")?.map(|local| local.view)))
}
pub(super) fn hold(rows: &Rows<'_>) -> Result<Option<Hold>> {
    rows.get(HOLD, "current")
}
pub(super) fn put_hold(rows: &Rows<'_>, hold: &Hold) -> Result<()> {
    rows.put(HOLD, "current", hold)
}
pub(super) fn clear_hold(rows: &Rows<'_>) -> Result<()> {
    rows.delete(HOLD, "current").map(|_| ())
}
pub(super) fn install(
    rows: &Rows<'_>,
    view: &View,
    claude: bool,
    refreeze: bool,
    fallback: Option<&PreparedBridge>,
) -> Result<()> {
    let previous = rows.get::<Local>(LOCAL, "current")?.and_then(|local| local.bridge);
    let fallback_used = previous.is_none();
    let previous = previous.or_else(|| fallback.map(|fallback| fallback.bridge.clone()));
    let bridge = if claude {
        let paused = view.pending.is_some()
            || view.goal.as_ref().is_some_and(|goal| goal.status == Status::Paused && goal.retired.is_none());
        let held = if !paused {
            Some(Held::Empty)
        } else if !refreeze && previous.as_ref().is_some_and(|bridge| bridge.paused && bridge.held.is_some()) {
            previous.and_then(|bridge| bridge.held)
        } else {
            Some(Held::Revision { revision: crate::inbox::presence::revision(rows)? })
        };
        Some(Bridge { active: view.active(), paused, held })
    } else {
        None
    };
    match bridge.as_ref().and_then(|bridge| bridge.held.as_ref()) {
        Some(Held::Revision { revision }) => {
            crate::inbox::presence::freeze(rows, Some(*revision))?;
            payloads::link(rows.connection(), rows.conversation(), HELD, "current", None)?;
        }
        Some(Held::Legacy) => {
            crate::inbox::presence::freeze(rows, None)?;
            if fallback_used && let Some(fallback) = fallback {
                fallback.install(rows)?;
            }
        }
        None | Some(Held::Empty) => {
            crate::inbox::presence::freeze(rows, None)?;
            payloads::link(rows.connection(), rows.conversation(), HELD, "current", None)?;
        }
    }
    rows.put(LOCAL, "current", &Local { view: view.clone(), bridge })?;
    rows.connection()
        .execute("INSERT OR IGNORE INTO goal_projection_dirty(conversation) VALUES(?1)", [rows.conversation()])?;
    if let Some(fallback) = fallback {
        fallback.finish(rows)?;
    }
    Ok(())
}
pub(super) fn projection(directory: &ConversationDir) -> io::Result<(u64, Option<HostBridge>)> {
    records::read(directory, |rows| {
        let version = version(rows)?;
        let Some(bridge) = rows.get::<Local>(LOCAL, "current")?.and_then(|local| local.bridge) else {
            return Ok((version, None));
        };
        let held = match bridge.held {
            Some(Held::Revision { revision }) => Some(crate::inbox::presence::held(rows, revision)?),
            Some(Held::Legacy) => {
                let bytes =
                    payloads::read_root(rows.connection(), rows.conversation(), HELD, "current")?.ok_or_else(|| {
                        fastctx_storage::Error::with_kind(
                            fastctx_storage::ErrorKind::Corrupt,
                            "frozen goal inputs are missing",
                        )
                    })?;
                Some(records::decode(&bytes)?)
            }
            Some(Held::Empty) => Some(Vec::new()),
            None => None,
        };
        Ok((version, Some(HostBridge { active: bridge.active, paused: bridge.paused, held })))
    })
}
fn version(rows: &Rows<'_>) -> Result<u64> {
    Ok(rows
        .connection()
        .query_row(
            "SELECT version FROM conversation_record_versions WHERE conversation=?1 AND family=?2",
            (rows.conversation(), LOCAL),
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0) as u64)
}
pub(super) fn projected(directory: &ConversationDir, expected: u64) -> io::Result<bool> {
    records::write_at(directory, Priority::Normal, 128, move |rows| {
        if version(rows)? != expected {
            return Ok(false);
        }
        rows.connection().execute("DELETE FROM goal_projection_dirty WHERE conversation=?1", [rows.conversation()])?;
        Ok(true)
    })
}

pub(super) fn registered(rows: &Rows<'_>, fallback: Option<&PreparedBridge>) -> Result<()> {
    if let Some(local) = rows.get::<Local>(LOCAL, "current")?
        && local.bridge.is_none()
    {
        install(rows, &local.view, true, false, fallback)?;
    }
    if let Some(fallback) = fallback {
        fallback.finish(rows)?;
    }
    Ok(())
}
