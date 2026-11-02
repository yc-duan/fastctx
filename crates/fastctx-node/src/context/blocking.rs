//! Async context preparation keeps synchronous record and projection work on
//! the blocking pool, including the first use of a newly born conversation.
use crate::Node;
use fastctx_types::ResidentId;
use std::{io, sync::Arc};

pub(super) async fn with<T: Send + 'static>(
    node: &Arc<Node>,
    id: &ResidentId,
    action: impl FnOnce(&Node, &ResidentId) -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    let (node, id) = (node.clone(), id.clone());
    fastctx_platform::task::spawn_blocking(move || action(&node, &id)).await.map_err(io::Error::other)?
}

pub(crate) async fn set_account(
    node: &Arc<Node>,
    id: &ResidentId,
    name: &str,
    text: &str,
    change: super::Change<'_>,
) -> io::Result<()> {
    let (name, text, by, version, fact, full) = (
        name.to_string(),
        text.to_string(),
        change.by.cloned(),
        change.version.to_string(),
        change.fact.to_string(),
        change.full,
    );
    with(node, id, move |node, id| {
        super::set_account(
            node,
            id,
            &name,
            &text,
            super::Change { by: by.as_ref(), version: &version, fact: &fact, full },
        )
    })
    .await
}
