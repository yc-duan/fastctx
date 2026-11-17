//! Legacy records migrate after the node opens, on bounded background workers.
pub(crate) mod archive;
#[cfg(feature = "storage-drill")]
pub(crate) mod drill;
pub(crate) mod errors;
pub(crate) mod files;
pub(crate) mod scheduler;
pub(crate) mod source;
pub(crate) mod withdrawal;
pub(crate) use files::{Context, FileComplete, FileRow, file};
pub(crate) use scheduler::Service;

/// What bringing the legacy files over keeps: a content key for each row
/// that had one and each file's progress, gone once the files are archived
/// (`maintenance::collect`) or their conversation goes, and the generation
/// each conversation was born in, a row for each.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS migration_content_keys (
    conversation TEXT NOT NULL, family TEXT NOT NULL, hash TEXT NOT NULL, occurrences INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(conversation,family,hash)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS migration_files (
    conversation TEXT NOT NULL, family TEXT NOT NULL, path TEXT NOT NULL,
    signature BLOB NOT NULL, through INTEGER NOT NULL DEFAULT 0, incarnation TEXT NOT NULL DEFAULT '',
    verified INTEGER NOT NULL DEFAULT 0, archived INTEGER NOT NULL DEFAULT 0, damage TEXT,
    PRIMARY KEY(family,path)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS migration_files_owner ON migration_files(conversation,family,path);
CREATE INDEX IF NOT EXISTS migration_files_archived ON migration_files(family,path,conversation) WHERE archived=1;
CREATE TABLE IF NOT EXISTS migration_born(name TEXT PRIMARY KEY NOT NULL,generation INTEGER NOT NULL) WITHOUT ROWID;
";

pub(crate) fn world(directory: &fastctx_types::layout::ConversationDir) -> std::io::Result<std::path::PathBuf> {
    directory
        .path()
        .parent()
        .and_then(std::path::Path::parent)
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| std::io::Error::other("conversation has no world"))
}

/// Notes a conversation this release is creating, before anything is kept
/// for it, and keeps the note: a restart does not take it for one holding
/// earlier records. A note not kept only has it looked at as one might be.
pub(crate) fn born(node: &crate::Node, id: &fastctx_types::ResidentId) {
    scheduler::born(node.world.path(), id.as_str());
    let (database, log, name) = (node.database.clone(), node.log.clone(), id.to_string());
    fastctx_platform::task::spawn_blocking(move || {
        let kept = database.write_relaxed_now_at(fastctx_storage::Priority::Background, name.len() + 32, move |db| {
            db.execute(
                "INSERT OR REPLACE INTO migration_born(name,generation) VALUES(?1,?2)",
                (&name, scheduler::BORN),
            )?;
            Ok(())
        });
        if let Err(error) = kept {
            log.line(format_args!("keeping a new conversation as born: {error}"));
        }
    });
}

/// A derived step that reads a legacy file nothing else uses sets a malformed
/// one aside with its reason, as a decoder's damaged file is, and goes on.
pub(crate) fn set_aside(
    directory: &fastctx_types::layout::ConversationDir,
    family: &str,
    path: &std::path::Path,
    error: &std::io::Error,
) -> std::io::Result<()> {
    let conversation = directory
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| std::io::Error::other("invalid conversation identity"))?;
    archive::damaged(&world(directory)?, conversation, family, path, &error.to_string())
}

pub(crate) fn failure(error: std::io::Error) -> fastctx_types::ipc::Failure {
    use fastctx_types::ipc::{CapacityReason, Failure};
    if busy(&error) {
        Failure::not_run(error.to_string()).capacity(CapacityReason::Pending)
    } else {
        Failure::new(error.to_string())
    }
}

/// Whether `error` says to try again later: records still moving in, or a
/// database too busy to take the work now, as it is or wrapped as it came.
pub(crate) fn busy(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<fastctx_storage::Error>())
            .is_some_and(|inner| inner.kind() == fastctx_storage::ErrorKind::Busy)
}

/// A database's error as a caller is answered: busy is tried again later.
pub(crate) fn storage_failure(doing: &str, error: fastctx_storage::Error) -> fastctx_types::ipc::Failure {
    use fastctx_types::ipc::{CapacityReason, Failure};
    if error.kind() == fastctx_storage::ErrorKind::Busy {
        Failure::not_run(format!("{doing}: {error}")).capacity(CapacityReason::Pending)
    } else {
        Failure::new(format!("{doing}: {error}"))
    }
}

mod catalog;
mod jobs;
mod maintenance;
/// The first listener poll starts one continuation; backup completion opens
/// migration later. Neither waiting nor bulk import blocks accepting requests.
pub(crate) async fn after_open<F: std::future::Future>(node: &std::sync::Arc<crate::Node>, accept: F) -> F::Output {
    let mut accept = std::pin::pin!(accept);
    std::future::poll_fn(|context| {
        let result = std::future::Future::poll(accept.as_mut(), context);
        if node.record_migration.listener_opened() {
            let fault = node.database.fault().cloned();
            let gate = node.database.format_upgrade_gate();
            let node = std::sync::Arc::downgrade(node);
            fastctx_platform::task::spawn(async move {
                if let Some(error) = fault {
                    if let Some(node) = node.upgrade() {
                        node.log.line(format_args!("migration remains closed: {error}"));
                    }
                    return;
                }
                let outcome = match gate {
                    Some(gate) => backup_settled(&node, gate).await,
                    None => Ok(()),
                };
                let Some(node) = node.upgrade() else {
                    return;
                };
                if let Err(error) = outcome {
                    node.log.line(format_args!(
                        "upgrade backup ended without a durable result: {error}; continuing migration"
                    ));
                }
                match node.record_migration.start() {
                    Ok(true) => start(&node),
                    Ok(false) => {}
                    Err(error) => node.log.line(format_args!("starting record migration: {error}")),
                }
            });
        }
        result
    })
    .await
}
/// Waits for the format backup by its deadline. Past the deadline the worker may
/// still own IO, so release is tracked asynchronously, holding neither a thread
/// nor a database, and bulk migration stays paused until it happens.
async fn backup_settled(
    node: &std::sync::Weak<crate::Node>,
    gate: fastctx_storage::format_upgrade::MigrationGate,
) -> fastctx_storage::Result<()> {
    let outcome = gate.wait().await;
    if outcome.is_ok() || gate.finished() {
        return outcome.map(|_| ());
    }
    if let Some(node) = node.upgrade() {
        node.log.line(format_args!(
            "backup deadline passed while IO is still active; bulk migration remains paused until handles are released"
        ));
    }
    gate.released().await.map(|_| ())
}
fn start(node: &std::sync::Arc<crate::Node>) {
    if node.database.fault().is_some() {
        return;
    }
    crate::activity_book::start(node);
    crate::timers::start(node);
    crate::letters::migrate(node);
    maintenance::start(node);
    let stopping = node.clone();
    fastctx_platform::task::spawn(async move {
        crate::on_stop::resume(&stopping).await;
        crate::run_stops::resume(&stopping);
    });
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        // What this release's migrations found is not looked for again at
        // every start, each look reading every conversation's place and
        // every job's path: only an import, which clears the mark
        // (`conversation_store::transfer`), or a release adding a migration,
        // which raises `BORN`, has discovery look again.
        let reading = node.clone();
        let discovered = fastctx_platform::task::spawn_blocking(move || {
            reading.database.read_background_now(|db| {
                use fastctx_storage::rusqlite::OptionalExtension;
                db.query_row("SELECT value FROM node_record_state WHERE name='legacy-discovery-complete'", [], |row| {
                    row.get::<_, i64>(0)
                })
                .optional()
                .map_err(Into::into)
            })
        })
        .await;
        if matches!(discovered, Ok(Ok(Some(generation))) if generation >= scheduler::BORN) {
            return;
        }
        let mut again = crate::again::Again::default();
        let mut catalog = catalog::Catalog::default();
        loop {
            let working = node.clone();
            let next = fastctx_platform::task::spawn_blocking(move || {
                let result = scheduler::discovery(|| catalog.scan(&working));
                (catalog, result)
            })
            .await;
            let result = match next {
                Ok((state, result)) => {
                    catalog = state;
                    Ok(result)
                }
                Err(error) => {
                    catalog = catalog::Catalog::default();
                    Err(error)
                }
            };
            match result {
                Ok(Ok(true)) => break,
                Ok(Ok(false)) => {
                    again.reset();
                    // The next pass waits for some family to move, and for
                    // the queue to drain to half, so each pass asks for a
                    // batch rather than for one place at a time.
                    let world = node.world.path();
                    loop {
                        tokio::select! {
                            () = node.record_migration.wait_progress() => {},
                            () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {},
                        }
                        if scheduler::queued(world) <= scheduler::DRAINED {
                            break;
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
                Ok(Err(error)) => node.log.line(format_args!("discovering legacy records: {error}")),
                Err(error) => node.log.line(format_args!("legacy discovery worker: {error}")),
            }
            tokio::time::sleep(again.next()).await;
        }
        let projecting = node.clone();
        let _ = fastctx_platform::task::spawn_blocking(move || crate::context::migrate(&projecting)).await;
        loop {
            let working = node.clone();
            let result = fastctx_platform::task::spawn_blocking(move || {
                working.database.write_now_at(fastctx_storage::Priority::Background, 64, |db| {
                    db.execute(
                        "INSERT INTO node_record_state(name,value) VALUES('legacy-discovery-complete',?1) ON CONFLICT(name) DO UPDATE SET value=?1",
                        [scheduler::BORN],
                    )?;
                    Ok(())
                }).map_err(std::io::Error::other)?;
                maintenance::collect(&working.database)
            })
            .await;
            match result {
                Ok(Ok(())) => break,
                Ok(Err(error)) => node.log.line(format_args!("finishing legacy discovery: {error}")),
                Err(error) => node.log.line(format_args!("discovery completion worker: {error}")),
            }
            tokio::time::sleep(again.next()).await;
        }
    });
}
