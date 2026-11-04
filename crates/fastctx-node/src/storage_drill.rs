//! Storage health drill entry point. No node, account or model is started.

pub fn run(root: &std::path::Path, family: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let _role = fastctx_platform::roles::scoped(fastctx_platform::roles::Role::Blocking);
    let violations = fastctx_platform::doing::violations();
    if !root.is_absolute() || fastctx_platform::disk::is_there(root) {
        return Err("the drill needs an absolute, absent root".into());
    }
    if family.is_some_and(|family| !matches!(family, "handovers" | "executor" | "facts" | "records")) {
        return Err("unknown record drill family".into());
    }
    if family.is_none() {
        crate::run_facts::migration_drill::run(&root.join("call-migration"))?;
        crate::activity_book::migration_drill::run(&root.join("book-migration"))?;
        crate::record_migration::drill::run(&root.join("migration"))?;
        crate::context::migration_drill::run(&root.join("context-withdrawal"))?;
        crate::gather::drill::run(&root.join("gather"))?;
        crate::conversation_store::drill::run(&root.join("conversations"))?;
        crate::conversation_store::transfer_drill::run(&root.join("transfer"))?;
        crate::record_payloads::drill::run(&root.join("payload-gc"))?;
    }
    if family.is_none_or(|family| family == "handovers") {
        crate::delivery::drill::run(&root.join("handovers"))?;
    }
    if family.is_none_or(|family| family == "executor") {
        crate::run_facts::drill_executor::run(&root.join("executor"))?;
    }
    if family.is_none_or(|family| family == "facts") {
        crate::run_facts::drill::run(&root.join("facts"))?;
    }
    if family.is_none_or(|family| family == "records") {
        crate::records::drill::run(&root.join("records"))?;
    }
    assert_eq!(fastctx_platform::doing::violations(), violations, "record drill violated lock order");
    Ok(())
}

/// Runs only migration admission, source verification and event-order checks.
pub fn migration(root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    if !root.is_absolute() || fastctx_platform::disk::is_there(root) {
        return Err("the drill needs an absolute, absent root".into());
    }
    crate::run_facts::migration_drill::run(&root.join("call-migration"))?;
    crate::activity_book::migration_drill::run(&root.join("book-migration"))?;
    crate::record_migration::drill::run(&root.join("migration"))?;
    crate::context::migration_drill::run(&root.join("context-withdrawal"))?;
    crate::gather::drill::run(&root.join("gather"))?;
    crate::session_record::drill::run(&root.join("events"))?;
    Ok(())
}

/// Runs the whole-conversation transport proof in a fresh private world.
pub fn transfer(root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    if !root.is_absolute() || fastctx_platform::disk::is_there(root) {
        return Err("the drill needs an absolute, absent root".into());
    }
    crate::conversation_store::transfer_drill::run(root)
}

/// How long an in-process restart waits for the old connections to close:
/// a shared Windows runner's disk took longer than ten seconds.
const CLOSING: std::time::Duration = std::time::Duration::from_secs(60);

/// How long it waits for the WAL to go before opening with it there: two
/// handles closing together can each find the other's lock at its final
/// checkpoint, which waits for none, and leave the WAL behind them with no
/// connection left to remove it, as a killed process does.
const WAL_GONE: std::time::Duration = std::time::Duration::from_secs(10);

/// In-process restarts wait for the old connections' final checkpoint and WAL
/// removal, a while; a WAL left without them is recovered as after a killed
/// process, and a connection still open is told by the database being busy.
pub(crate) fn reopen(world: &fastctx_types::layout::WorldDir) -> fastctx_storage::Result<fastctx_storage::Database> {
    let started = std::time::Instant::now();
    loop {
        if started.elapsed() < WAL_GONE && fastctx_platform::disk::exists(world.path().join("node.sqlite3-wal"))? {
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }
        let database = crate::record_database::open(world);
        match database.fault().cloned() {
            None => return Ok(database),
            Some(error) if error.kind() == fastctx_storage::ErrorKind::Busy && started.elapsed() < CLOSING => {
                drop(database);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Some(error) => return Err(error),
        }
    }
}
