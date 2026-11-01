//! Storage health drill entry point. No node, account or model is started.

pub fn run(root: &std::path::Path, family: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let _role = fastctx_platform::roles::scoped(fastctx_platform::roles::Role::Blocking);
    let violations = fastctx_platform::doing::violations();
    if !root.is_absolute() || root.exists() {
        return Err("the drill needs an absolute, absent root".into());
    }
    if family.is_some_and(|family| !matches!(family, "handovers" | "executor" | "facts" | "records")) {
        return Err("unknown record drill family".into());
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
