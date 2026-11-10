//! User opening and speaking calls, recorded before their host work begins.

mod colleagues;
mod opening;
mod speaking;

pub(crate) use colleagues::{ask, ask_here};
pub(crate) use opening::{open, open_here};
pub(crate) use speaking::{message_here, talk, talk_here};

use std::sync::Arc;

use crate::Node;

pub(crate) fn register(node: &Arc<Node>) {
    opening::register(node);
    speaking::register(node);
    colleagues::register(node);
}
