//! Real writer lifecycle checks. Every run requires an explicit disposable root.
#[path = "sync_drill/admission.rs"]
mod admission;
#[path = "sync_drill/blocks.rs"]
mod blocks;
#[path = "sync_drill/format_upgrade.rs"]
mod format_upgrade;
#[path = "sync_drill/scheduler.rs"]
mod scheduler;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Windows gives the entry thread 1 MiB, which the combined debug fixture
    // polls can exceed. Match the runtime workers' 2 MiB instead.
    if cfg!(windows) {
        std::thread::Builder::new()
            .name("sync-drill".into())
            .stack_size(2 * 1024 * 1024)
            .spawn(|| run().map_err(|error| error.to_string()))?
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            .map_err(Into::into)
    } else {
        run()
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os().nth(1).map(std::path::PathBuf::from).ok_or("usage: sync_drill DISPOSABLE_ROOT")?;
    if std::env::args().nth(2).as_deref() == Some("replica-child") {
        return blocks::replica::child(&root, std::env::args().nth(3).ok_or("missing crash point")?.parse()?);
    }
    if std::env::args().nth(2).as_deref() == Some("follow-child") {
        return blocks::source_acceptance::follow::child(&root);
    }
    if std::env::args().nth(2).as_deref() == Some("upload-child") {
        return blocks::source_acceptance::upload::child(&root);
    }
    if root.exists() {
        return Err("the drill root must not exist".into());
    }
    fastctx_platform::ensure_private_dir(&root)?;
    if std::env::args().nth(2).as_deref() == Some("timeline") {
        return tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?
            .block_on(blocks::run_timeline(&root));
    }
    if std::env::args().nth(2).as_deref() == Some("upload-bench") {
        return tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?
            .block_on(blocks::source_acceptance::upload::benchmark(&root));
    }
    if std::env::args().nth(2).as_deref() == Some("upload") {
        return tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?
            .block_on(blocks::source_acceptance::upload::run(&root));
    }
    format_upgrade::run(&root)?;
    if std::env::args().nth(2).as_deref() == Some("upgrade") {
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?.block_on(async {
        admission::run().await?;
        scheduler::run(&root).await?;
        blocks::run(&root).await
    })?;
    Ok(())
}
