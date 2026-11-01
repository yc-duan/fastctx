//! Controls are individual records, with their completion event in the same commit.

use super::*;
use crate::records;
use fastctx_types::layout::ConversationDir;
use serde::de::{self, Deserializer as _, SeqAccess, Visitor};
use std::io;

const FAMILY: &str = "controls";
const RESULTS: &str = "control-results";

pub(crate) fn initialize(directory: &ConversationDir) -> io::Result<()> {
    records::ensure_file(directory, FAMILY, directory.path().join("control-queue.json"), |path, push| {
        let file = match fastctx_platform::disk::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        struct Controls<'a> {
            path: std::path::PathBuf,
            push: &'a mut dyn FnMut(records::Source) -> io::Result<()>,
        }
        impl<'de> Visitor<'de> for Controls<'_> {
            type Value = ();
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an array of pending controls")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut rows: A) -> Result<(), A::Error> {
                while let Some(raw) = rows.next_element::<Box<serde_json::value::RawValue>>()? {
                    let row: PendingControl = serde_json::from_str(raw.get()).map_err(de::Error::custom)?;
                    let body = raw.get().as_bytes().to_vec();
                    let source =
                        records::Source::new(row.id.clone(), self.path.clone(), body).map_err(de::Error::custom)?;
                    (self.push)(source).map_err(de::Error::custom)?;
                }
                Ok(())
            }
        }
        let mut decoder = serde_json::Deserializer::from_reader(io::BufReader::new(file));
        decoder.deserialize_seq(Controls { path: path.to_path_buf(), push }).map_err(io::Error::other)?;
        decoder.end().map_err(io::Error::other)
    })?;
    crate::session_record::prepare(directory)
}

async fn work<T: Send + 'static>(
    node: &Node,
    id: &ResidentId,
    action: impl FnOnce(&ConversationDir) -> io::Result<T> + Send + 'static,
) -> Result<T, Failure> {
    let directory = node.world.conversation(id);
    fastctx_platform::task::spawn_blocking(move || {
        initialize(&directory)?;
        action(&directory)
    })
    .await
    .map_err(|error| failure(io::Error::other(error)))?
    .map_err(failure)
}

pub(crate) async fn pending(node: &Node, id: &ResidentId) -> Result<Vec<PendingControl>, Failure> {
    work(node, id, |directory| {
        records::read(directory, |rows| {
            let mut result = Vec::new();
            let mut cursor = 0;
            loop {
                let page = rows.page::<PendingControl>(FAMILY, cursor, 128)?;
                let Some(last) = page.last() else { break };
                cursor = last.0;
                result.extend(page.into_iter().map(|(_, row)| row));
            }
            Ok(result)
        })
    })
    .await
}

pub(super) async fn put(node: &Node, id: &ResidentId, row: PendingControl) -> Result<(), Failure> {
    work(node, id, move |directory| {
        let bytes = serde_json::to_vec(&row)?.len();
        transaction(directory, bytes, move |rows| {
            if rows.get::<PendingControl>(RESULTS, &row.id)?.is_some() {
                return Err(fastctx_storage::Error::new("the control has already ended"));
            }
            rows.put(FAMILY, &row.id, &row)
        })
    })
    .await
}

pub(super) async fn running(node: &Node, id: &ResidentId, key: &str) -> Result<(), Failure> {
    let key = key.to_string();
    work(node, id, move |directory| {
        transaction(directory, key.len(), move |rows| {
            let mut row = rows
                .get::<PendingControl>(FAMILY, &key)?
                .ok_or_else(|| fastctx_storage::Error::new("the control was withdrawn"))?;
            if row.state != ControlState::Waiting {
                return Err(fastctx_storage::Error::new("the control is no longer waiting"));
            }
            row.state = ControlState::Running;
            rows.put(FAMILY, &key, &row)
        })
    })
    .await
}

pub(super) async fn cancel(node: &Arc<Node>, id: &ResidentId, key: &str) -> Result<PendingControl, Failure> {
    let key = key.to_string();
    let watched = node.clone();
    let recipient = id.clone();
    work(node, id, move |directory| {
        let (row, event, changed) = transaction(directory, key.len(), move |rows| {
            let mut row = rows
                .get::<PendingControl>(FAMILY, &key)?
                .ok_or_else(|| fastctx_storage::Error::new("no waiting control with this id"))?;
            if row.state != ControlState::Waiting {
                return Err(fastctx_storage::Error::new(
                    "this control has already entered the host and cannot be withdrawn",
                ));
            }
            row.state = ControlState::Cancelled;
            let event = event_of(&row);
            let key = format!("{}:{:?}", row.id, row.state);
            rows.put_until(
                RESULTS,
                &row.id,
                &row,
                Some(jiff::Timestamp::now() + jiff::SignedDuration::from_hours(30 * 24)),
            )?;
            let changed = rows.event(&key, &event)?;
            rows.delete(FAMILY, &row.id)?;
            Ok((row, event, changed))
        })?;
        if changed {
            watched.watchers.publish(&recipient, &event);
        }
        Ok(row)
    })
    .await
}

fn event_of(row: &PendingControl) -> SessionEvent {
    SessionEvent {
        at: jiff::Timestamp::now(),
        happened: Happened::Changed {
            turn: None,
            change: SessionChange::Metadata { metadata: serde_json::json!({"fastctx_control":row}) },
        },
    }
}

pub(super) async fn tell(
    node: &Arc<Node>,
    id: &ResidentId,
    row: &PendingControl,
    remove: bool,
    retain: bool,
) -> Result<(), Failure> {
    let (node, id, row) = (node.clone(), id.clone(), row.clone());
    let directory = node.world.conversation(&id);
    fastctx_platform::task::spawn_blocking(move || {
        initialize(&directory)?;
        let bytes = serde_json::to_vec(&row)?.len() + 512;
        let (event, changed) = transaction(&directory, bytes, move |rows| {
            let prior = rows.get::<PendingControl>(RESULTS, &row.id)?;
            let ended = prior.is_some();
            let row = prior.unwrap_or(row);
            if retain && !ended {
                rows.put(FAMILY, &row.id, &row)?;
            }
            let event = event_of(&row);
            let key = format!("{}:{:?}", row.id, row.state);
            if remove {
                rows.put_until(
                    RESULTS,
                    &row.id,
                    &row,
                    Some(jiff::Timestamp::now() + jiff::SignedDuration::from_hours(30 * 24)),
                )?;
            }
            let changed = rows.event(&key, &event)?;
            if remove {
                rows.delete(FAMILY, &row.id)?;
            }
            Ok((event, changed))
        })?;
        if changed {
            node.watchers.publish(&id, &event);
        }
        Ok::<_, io::Error>(())
    })
    .await
    .map_err(|error| failure(io::Error::other(error)))?
    .map_err(failure)
}

fn transaction<T: Send + 'static>(
    directory: &ConversationDir,
    bytes: usize,
    action: impl FnOnce(&records::Rows<'_>) -> fastctx_storage::Result<T> + Send + 'static,
) -> io::Result<T> {
    records::write_at(directory, fastctx_storage::Priority::Interactive, bytes, action)
}
