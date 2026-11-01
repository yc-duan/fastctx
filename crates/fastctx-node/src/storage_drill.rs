//! Storage health drill entry point. No node, account or model is started.

pub fn run(root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    if !root.is_absolute() || root.exists() {
        return Err("the drill needs an absolute, absent root".into());
    }
    crate::conversation_store::drill::run(&root.join("conversations"))?;
    crate::run_facts::drill::run(&root.join("facts"))?;
    crate::records::drill::run(&root.join("records"))?;
    Ok(())
}
