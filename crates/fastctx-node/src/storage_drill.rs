//! Storage health drill entry point. No node, account or model is started.

pub fn run(root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    if !root.is_absolute() || root.exists() {
        return Err("the drill needs an absolute, absent root".into());
    }
    crate::run_facts::migration_drill::run(&root.join("call-migration"))?;
    crate::activity_book::migration_drill::run(&root.join("book-migration"))?;
    crate::record_migration::drill::run(&root.join("migration"))?;
    crate::context::migration_drill::run(&root.join("context-withdrawal"))?;
    crate::gather::drill::run(&root.join("gather"))?;
    crate::conversation_store::drill::run(&root.join("conversations"))?;
    crate::conversation_store::transfer_drill::run(&root.join("transfer"))?;
    crate::record_payloads::drill::run(&root.join("payload-gc"))?;
    crate::run_facts::drill::run(&root.join("facts"))?;
    crate::records::drill::run(&root.join("records"))?;
    Ok(())
}

/// Runs only migration admission, source verification and event-order checks.
pub fn migration(root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    if !root.is_absolute() || root.exists() {
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
    if !root.is_absolute() || root.exists() {
        return Err("the drill needs an absolute, absent root".into());
    }
    crate::conversation_store::transfer_drill::run(root)
}

/// Dropping the last handle asks the SQLite writer to close asynchronously.
/// In-process restart drills wait for that close; a killed process has no such
/// surviving thread. Only the engine's explicit Busy result is retried.
pub(crate) fn reopen(world: &fastctx_types::layout::WorldDir) -> fastctx_storage::Result<fastctx_storage::Database> {
    let started = std::time::Instant::now();
    loop {
        let database = crate::record_database::open(world);
        match database.fault().cloned() {
            None => return Ok(database),
            Some(error)
                if error.kind() == fastctx_storage::ErrorKind::Busy
                    && started.elapsed() < std::time::Duration::from_secs(10) =>
            {
                drop(database);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Some(error) => return Err(error),
        }
    }
}
