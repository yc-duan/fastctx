//! Submitted changes to records and configuration, without recording credentials.

mod accounts;
mod data;
mod scope;

use crate::Node;
use fastctx_types::{
    Call, Reply,
    ipc::{Failure, Peer},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// The entrances whitelist calls before constructing Asked. Credential-bearing
// profile, MCP and arbitrary filesystem writes never enter these recordings.
#[derive(Serialize, Deserialize)]
struct Asked {
    peer: Peer,
    call: Box<Call>,
}

pub(crate) fn register(node: &Arc<Node>) {
    accounts::register(node);
    data::register(node);
}

pub(crate) fn handles(call: &Call) -> bool {
    accounts::handles(call)
        || matches!(
            call,
            Call::Data(fastctx_types::data::DataCall::Delete { .. } | fastctx_types::data::DataCall::ClearCache)
        )
}

pub(crate) async fn local(node: &Arc<Node>, peer: &Peer, call: Call) -> Result<Reply, Failure> {
    match call {
        Call::Data(_) => data::local(node, peer, call).await,
        _ => accounts::local(node, peer, call).await,
    }
}

fn unit(value: &impl Serialize) -> Result<String, Failure> {
    let bytes = serde_json::to_vec(value).map_err(|error| Failure::not_run(error.to_string()))?;
    Ok(fastctx_hands::files::content_hash(&bytes))
}
