//! Sentinel submissions retain caller facts without persisting credential values.
mod configuration;
mod controls;

use crate::Node;
pub(super) use configuration::{delete_key, settings, tried};
pub(super) use controls::{control, home};
use std::sync::Arc;

pub(crate) fn register(node: &Arc<Node>) {
    controls::register(node);
    configuration::register(node);
}

/// Local units include the hub and account without depending on a display name's length.
fn scoped_unit<T: serde::Serialize>(kind: &str, value: &T) -> String {
    let bytes = serde_json::to_vec(value).expect("submission identities serialize");
    format!("{kind}/{}", fastctx_hands::files::content_hash(&bytes))
}
