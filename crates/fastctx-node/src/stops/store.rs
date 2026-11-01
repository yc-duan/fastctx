//! Input stop obligations remain until the exact consuming turn is stopped or settled.
use crate::{leases, records};
use fastctx_storage::Priority;
use fastctx_types::{NoticeId, layout::ConversationDir, turn_evidence::StopSource};
use std::{
    io,
    path::PathBuf,
    sync::{Arc, LazyLock},
};

type Gate = Arc<tokio::sync::Mutex<()>>;
static GATES: LazyLock<leases::Registry<PathBuf, Gate>> = LazyLock::new(Default::default);
const FAMILY: &str = "input-stops";
pub(super) struct Order {
    _lease: leases::Lease<PathBuf, Gate>,
    _permit: tokio::sync::OwnedMutexGuard<()>,
}

/// The worker owns ordering until it finishes, even if its caller is cancelled.
pub(super) async fn ordered<T: Send + 'static>(
    directory: ConversationDir,
    action: impl FnOnce(&ConversationDir, &Arc<Order>) -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    let lease = GATES.get_or_insert_with(&directory.path().to_path_buf(), || Arc::new(tokio::sync::Mutex::new(())));
    let permit = Arc::clone(&lease).lock_owned().await;
    let order = Arc::new(Order { _lease: lease, _permit: permit });
    fastctx_platform::task::spawn_blocking(move || action(&directory, &order)).await.map_err(io::Error::other)?
}

pub(crate) fn prepare(directory: &ConversationDir) -> io::Result<()> {
    let folder = directory.book("input-stops");
    records::ensure_each(directory, FAMILY, move |push| {
        let entries = match fastctx_platform::disk::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let Some(key) = entry.file_name().to_str().and_then(|key| key.parse::<NoticeId>().ok()) else { continue };
            crate::record_migration::file(&entry.path(), push, |push| {
                let bytes = fastctx_platform::disk::read(entry.path())?;
                let body = if bytes.is_empty() {
                    serde_json::to_vec(&StopSource::User)?
                } else {
                    let _: StopSource = serde_json::from_slice(&bytes)?;
                    bytes
                };
                push(records::Source::new(key.to_string(), entry.path(), body)?)
            })?;
        }
        Ok(())
    })
}

pub(super) fn keep(
    directory: &ConversationDir,
    inputs: &[NoticeId],
    source: StopSource,
    order: &Arc<Order>,
    committed: &Arc<dyn Fn() + Send + Sync>,
) -> io::Result<()> {
    prepare(directory)?;
    for batch in inputs.chunks(16) {
        let batch = batch.to_vec();
        let order = order.clone();
        let committed = committed.clone();
        records::write_then_at(
            directory,
            Priority::Control,
            batch.iter().map(|id| id.as_str().len() + 64).sum(),
            move |rows| {
                for input in batch {
                    if rows.get::<StopSource>(FAMILY, input.as_str())? != Some(StopSource::User) {
                        rows.put(FAMILY, input.as_str(), &source)?;
                    }
                }
                Ok(())
            },
            move |_| {
                committed();
                drop(order);
            },
        )?;
    }
    Ok(())
}

pub(super) fn source(directory: &ConversationDir, inputs: &[NoticeId]) -> io::Result<Option<StopSource>> {
    prepare(directory)?;
    records::read_background(directory, |rows| {
        let mut result = None;
        for input in inputs {
            if let Some(source) = rows.get::<StopSource>(FAMILY, input.as_str())? {
                result = Some(source);
            }
            if result == Some(StopSource::User) {
                break;
            }
        }
        Ok(result)
    })
}

/// A stronger stop arriving during host interruption is retained for the next attempt.
pub(super) fn finish(
    directory: &ConversationDir,
    inputs: &[NoticeId],
    applied: StopSource,
    order: &Arc<Order>,
) -> io::Result<bool> {
    prepare(directory)?;
    let mut finished = true;
    for batch in inputs.chunks(16) {
        let batch = batch.to_vec();
        let order = order.clone();
        finished &= records::write_then_at(
            directory,
            Priority::Control,
            batch.iter().map(|id| id.as_str().len() + 64).sum(),
            move |rows| {
                let mut finished = true;
                for input in batch {
                    if applied != StopSource::User
                        && rows.get::<StopSource>(FAMILY, input.as_str())? == Some(StopSource::User)
                    {
                        finished = false;
                    } else {
                        rows.delete(FAMILY, input.as_str())?;
                    }
                }
                Ok(finished)
            },
            move |_| drop(order),
        )?;
    }
    Ok(finished)
}

pub(super) fn pending(directory: &ConversationDir) -> io::Result<Vec<NoticeId>> {
    prepare(directory)?;
    let mut after = String::new();
    let mut inputs = Vec::new();
    loop {
        let page = records::read(directory, |rows| rows.key_page::<StopSource>(FAMILY, &after, 128))?;
        let Some(last) = page.last() else { break };
        after = last.0.clone();
        for (key, _) in page {
            inputs.push(key.parse().map_err(io::Error::other)?);
        }
    }
    Ok(inputs)
}

pub(super) fn has_pending(directory: &ConversationDir) -> io::Result<bool> {
    prepare(directory)?;
    records::read(directory, |rows| {
        Ok(rows.connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM conversation_records WHERE conversation=?1 AND family='input-stops')",
            [rows.conversation()],
            |row| row.get(0),
        )?)
    })
}
