//! Real writer lifecycle checks. Every run requires an explicit disposable root.
#[path = "sync_drill/scheduler.rs"]
mod scheduler;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os().nth(1).map(std::path::PathBuf::from).ok_or("usage: sync_drill DISPOSABLE_ROOT")?;
    if root.exists() {
        return Err("the drill root must not exist".into());
    }
    fastctx_platform::ensure_private_dir(&root)?;
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(scheduler::run(&root))?;
    Ok(())
}
