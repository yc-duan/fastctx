//! What this machine is on each network as a conversation is born here:
//! shared whole as a body, or not (design §29, 2.1). Kept in the
//! conversation's `about` at its birth, so the scope it is placed in follows
//! what the machine knew then, however late its first scope is made and
//! wherever it moved since.

use crate::Node;
use std::collections::BTreeMap;

/// The body this machine is on each network it is on now, or none where it
/// is not shared whole, by hub id.
pub(crate) fn now(node: &Node) -> BTreeMap<String, Option<String>> {
    node.links()
        .into_iter()
        .map(|link| {
            let (view, me) = (link.view(), node.machine());
            let directory = &view.directory;
            let machine = directory.canonical(&me);
            let shared = directory.shares.iter().any(|share| &share.machine == machine && share.active());
            (link.membership().id.to_string(), if shared { directory.body_of(machine) } else { None })
        })
        .collect()
}

/// The body this machine is on `hub` while it is shared whole there now.
pub(crate) fn shared_body(node: &Node, hub: &fastctx_types::access::HubId) -> Option<String> {
    now(node).remove(hub.as_str()).flatten()
}

/// The body this machine is on `hub` now, shared whole or not.
pub(crate) fn body_here(node: &Node, hub: &fastctx_types::access::HubId) -> Option<String> {
    let link = node.link_of(hub)?;
    let (view, me) = (link.view(), node.machine());
    let directory = &view.directory;
    directory.body_of(directory.canonical(&me))
}
