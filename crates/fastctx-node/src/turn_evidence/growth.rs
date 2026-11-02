//! Output counters advance atomically with their complete measurement snapshot.
use crate::records::{self, maps};
use fastctx_storage::{Error, ErrorKind, Priority};
use fastctx_types::{layout::ConversationDir, turn_evidence::WaitHandle};
use std::{collections::BTreeMap, io};

pub(super) async fn measure(directory: ConversationDir, handles: Vec<WaitHandle>) -> io::Result<Vec<WaitHandle>> {
    let (database, scope) = records::locate(&directory)?;
    let (mut handles, values) = tokio::task::spawn_blocking(move || {
        let values = handles
            .iter()
            .filter_map(|handle| {
                handle.output.as_ref().map(|output| (key(handle), output.bytes.to_be_bytes().to_vec()))
            })
            .collect();
        (handles, values)
    })
    .await
    .map_err(io::Error::other)?;
    let (_, handles) = maps::stage(
        database,
        scope,
        "waiting-output".into(),
        values,
        Priority::Interactive,
        move |previous| {
            let legacy;
            let previous = match previous {
                Some(previous) => previous,
                None => {
                    legacy = match std::fs::read(directory.books().join("waiting-output.json")) {
                        Ok(bytes) => serde_json::from_slice::<BTreeMap<String, u64>>(&bytes)
                            .map_err(|error| Error::with_kind(ErrorKind::Corrupt, error.to_string()))?
                            .into_iter()
                            .map(|(key, value)| (key, value.to_be_bytes().to_vec()))
                            .collect(),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => maps::Values::new(),
                        Err(error) => return Err(Error::new(error.to_string())),
                    };
                    &legacy
                }
            };
            for handle in &mut handles {
                if let Some(output) = &handle.output {
                    handle.output_growth = previous
                        .get(&key(handle))
                        .map(|bytes| {
                            let bytes: [u8; 8] = bytes
                                .as_slice()
                                .try_into()
                                .map_err(|_| Error::with_kind(ErrorKind::Corrupt, "invalid saved output counter"))?;
                            Ok::<_, Error>(output.bytes.saturating_sub(u64::from_be_bytes(bytes)))
                        })
                        .transpose()?;
                }
            }
            Ok(handles)
        },
        |rows, staged| {
            maps::link(rows, "current", staged)?;
            maps::finish(rows, staged)
        },
    )
    .await
    .map_err(io::Error::other)?;
    Ok(handles)
}

/// What a wait's output count is kept under. A job's short id is given again
/// once its files are gone: its completion, when it began or where its log is
/// tells one run from the next, so a later job does not take an earlier one's
/// count as its own.
fn key(handle: &WaitHandle) -> String {
    let run = handle
        .completion
        .as_ref()
        .map(ToString::to_string)
        .or_else(|| handle.since.map(|since| since.to_string()))
        .or_else(|| handle.output.as_ref().map(|output| output.path.clone()))
        .unwrap_or_default();
    format!("{}:{}:{run}", handle.machine, handle.id)
}
