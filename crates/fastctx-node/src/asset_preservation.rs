//! Migration diagnostics read indexed scope totals, never retained source bodies.

pub(crate) fn guard(
    db: &fastctx_storage::rusqlite::Connection,
) -> fastctx_storage::Result<Box<dyn fastctx_storage::Guard>> {
    Ok(Box::new(fastctx_storage::guard::CompositeGuard::new(vec![
        crate::conversation_store::fences::guard(db)?,
        fastctx_sync::migration::guard(db)?,
    ])))
}

pub(crate) fn report(node: &crate::Node) {
    if let Err(error) = node.database.read_background_now(fastctx_sync::migration::report) {
        node.log.line(format_args!("reading retained migration sources: {error}"));
    }
}
