//! User opening and asking calls, recorded before their host work begins.
//! Speaking is not among them: words to a resident answer once the node has
//! them, without a durable operation in their way.

mod colleagues;
mod opening;

pub(crate) use colleagues::{ask, ask_here};
pub(crate) use opening::{open, open_here};

use std::sync::Arc;

use crate::Node;

pub(crate) fn register(node: &Arc<Node>) {
    opening::register(node);
    colleagues::register(node);
}
