//! The project's memory as text for the named-context "map" block
//! (`crate::context::sources::map`). The project instructions ride the
//! "project" block, so this carries only the journals and decisions the
//! project keeps.

use crate::Node;
use fastctx_types::{TownName, access::Acting, ipc::Failure, memory::Catalog};
use std::sync::Arc;

/// The memory index for a project with the catalog it lists, or None when the
/// account has memory off. `read_only` leaves out what a colleague may not write.
pub(crate) async fn text(
    node: &Arc<Node>,
    session: Option<(&fastctx_types::ResidentId, &fastctx_types::residents::Origin)>,
    town: &TownName,
    acting: &Acting,
    read_only: bool,
) -> Result<Option<(String, Catalog)>, Failure> {
    if !super::settings::enabled_for(node, acting, town)? {
        return Ok(None);
    }
    let catalog = super::api::catalog_for(node, acting, town).await?;
    let folders = super::folders::get(node, session, town, acting).await;
    let text = fastctx_hands::memory::map(&catalog, folders.as_ref().map_err(String::as_str), read_only);
    Ok(Some((text, catalog)))
}
