//! `fastctx history delete PATH`: a conversation deletes one of its history
//! documents for good, naming its file as `write` and `edit` do. Writing the
//! file empty only takes the document down, for the app to bring back.

use std::{path::PathBuf, sync::Arc};

use crate::{Node, town::caller};
use fastctx_types::{
    history::HistoryReply,
    ipc::{Failure, Peer},
};

/// What a conversation is told that deletes a document it never read.
pub(crate) const UNSEEN: &str =
    "deleting removes this history document for good, every version of it; view it with fastctx view first";

/// What a conversation is told that deletes a document taken down.
pub(crate) const RETIRED: &str =
    "this history document was taken down and is not here to read first; the user can delete it for good in the app";

pub(crate) async fn command(node: &Arc<Node>, peer: &Peer, path: PathBuf) -> Result<HistoryReply, Failure> {
    let caller = caller::of(node, peer)?;
    node.authorize_here(&crate::around::acting_of(node, &caller))?;
    if let Some(reply) = crate::replicas::writing::delete(node, &caller, &path).await? {
        return Ok(reply);
    }
    if let Some(reply) = super::local_copy::delete(node, &caller, &path).await? {
        return Ok(reply);
    }
    Err(Failure::new(format!("{} is not a history document's file", path.display())))
}
