//! Explicit refusal for the retired journal protocol used by older clients and pages.

use crate::{Node, town::caller::Caller};
use fastctx_types::{JournalCall, Reply, ipc::Failure};
use std::sync::Arc;

pub async fn call(_: &Arc<Node>, _: &Caller, _: JournalCall) -> Result<Reply, Failure> {
    Err(Failure::new("the old journal interface was retired; update the client and use the project's Markdown memory"))
}
