//! Legacy records migrate after the node opens, on bounded background workers.
pub(crate) mod archive;
#[cfg(feature = "storage-drill")]
pub(crate) mod drill;
pub(crate) mod files;
pub(crate) mod scheduler;
pub(crate) mod source;
pub(crate) mod withdrawal;
pub(crate) use files::{Context, FileComplete, FileRow, file};
pub(crate) use scheduler::Service;

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
";

pub(crate) fn world(directory: &fastctx_types::layout::ConversationDir) -> std::io::Result<std::path::PathBuf> {
    directory
        .path()
        .parent()
        .and_then(std::path::Path::parent)
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| std::io::Error::other("conversation has no world"))
}

pub(crate) fn failure(error: std::io::Error) -> fastctx_types::ipc::Failure {
    use fastctx_types::ipc::{CapacityReason, Failure};
    if error.kind() == std::io::ErrorKind::WouldBlock {
        Failure::not_run(error.to_string()).capacity(CapacityReason::Pending)
    } else {
        Failure::new(error.to_string())
    }
}

mod catalog;
mod jobs;
mod maintenance;
/// The first actual poll of the listener opens background migration. A bound
/// socket alone does not prove that the node is servicing requests yet.
pub(crate) async fn after_open<F: std::future::Future>(node: &std::sync::Arc<crate::Node>, accept: F) -> F::Output {
    let mut accept = std::pin::pin!(accept);
    std::future::poll_fn(|context| {
        let result = std::future::Future::poll(accept.as_mut(), context);
        match node.record_migration.start() {
            Ok(true) => start(node),
            Ok(false) => {}
            Err(error) => node.log.line(format_args!("starting record migration: {error}")),
        }
        result
    })
    .await
}
fn start(node: &std::sync::Arc<crate::Node>) {
    if node.database.fault().is_some() {
        return;
    }
    crate::activity_book::start(node);
    crate::timers::start(node);
    crate::letters::migrate(node);
    let stopping = node.clone();
    fastctx_platform::task::spawn(async move {
        crate::on_stop::resume(&stopping).await;
        crate::run_stops::resume(&stopping);
    });
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        let reading = node.clone();
        let _ = fastctx_platform::task::spawn_blocking(move || archive::damaged_count(reading.world.path())).await;
        let mut again = crate::again::Again::default();
        loop {
            let working = node.clone();
            let result =
                fastctx_platform::task::spawn_blocking(move || scheduler::discovery(|| catalog::scan(&working))).await;
            match result {
                Ok(Ok(true)) => break,
                Ok(Ok(false)) => {
                    again.reset();
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    tokio::select! {
                        () = node.record_migration.wait_progress() => {},
                        () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {},
                    }
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
                maintenance::collect(&working.database)?;
                working.database.write_now_at(fastctx_storage::Priority::Background, 64, |db| {
                    db.execute("INSERT INTO node_record_state(name,value) VALUES('legacy-discovery-complete',1) ON CONFLICT(name) DO UPDATE SET value=1", [])?;
                    Ok(())
                }).map_err(std::io::Error::other)?;
                archive::damaged_count(working.world.path())?;
                archive::collect(working.world.path(), archive::RETENTION)
            })
            .await;
            match result {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => node.log.line(format_args!("maintaining legacy archives: {error}")),
                Err(error) => node.log.line(format_args!("archive maintenance worker: {error}")),
            }
            tokio::time::sleep(std::time::Duration::from_secs(60 * 60)).await;
        }
    });
}
