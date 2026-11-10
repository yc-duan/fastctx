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
    let root = std::env::args_os().nth(1).map(std::path::PathBuf::from).ok_or("usage: sync_drill DISPOSABLE_ROOT")?;
    if std::env::args().nth(2).as_deref() == Some("replica-child") {
        return blocks::replica::child(&root, std::env::args().nth(3).ok_or("missing crash point")?.parse()?);
    }
    if std::env::args().nth(2).as_deref() == Some("follow-child") {
        return blocks::source_acceptance::follow::child(&root);
    }
    if root.exists() {
        return Err("the drill root must not exist".into());
    }
    fastctx_platform::ensure_private_dir(&root)?;
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
