//! The conversations of a machine that is away, as this machine's copies of
//! them have them (plan 3-5, design §28.32). When the hub tells nothing of
//! what was afoot there — it was started again since the machine left — a
//! machine's list is made of what is held here: each conversation's
//! description, project, title, and when it was archived; none of them is
//! here or doing anything. Reads only read.

use crate::Node;
use fastctx_types::MachineName;
use fastctx_types::town::WhoReply;

/// The conversations `machine` keeps, as the copies here have them; none
/// when no copy here says it lives there.
pub(crate) fn of(node: &Node, machine: &MachineName) -> Option<WhoReply> {
    let library = &node.assets.as_ref()?.library;
    let scopes: Vec<_> =
        library.held_scopes().ok()?.into_iter().filter(|scope| super::copy::read_here(node, scope)).collect();
    fastctx_client::timeline_copy::listed(library, &scopes, machine)
}
