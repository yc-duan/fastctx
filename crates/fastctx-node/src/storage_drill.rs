//! Storage health drill entry point. No node, account or model is started.

pub fn run(root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    if !root.is_absolute() || root.exists() {
        return Err("the drill needs an absolute, absent root".into());
    }
    crate::run_facts::migration_drill::run(&root.join("call-migration"))?;
    crate::activity_book::migration_drill::run(&root.join("book-migration"))?;
    crate::record_migration::drill::run(&root.join("migration"))?;
    crate::conversation_store::drill::run(&root.join("conversations"))?;
    crate::conversation_store::transfer_drill::run(&root.join("transfer"))?;
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
