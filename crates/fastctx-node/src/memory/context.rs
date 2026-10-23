//! The memory producer for the named-context integration seam.
#![allow(dead_code, reason = "The embedding runtime supplies the persistent-context sink.")]

use crate::Node;
use fastctx_types::{
    Address, ResidentId, TownName,
    access::Acting,
    ipc::Failure,
    memory::{Catalog, Folders},
    residents::Origin,
};
use std::sync::Arc;

pub(crate) const BLOCK: &str = "map";
pub(crate) struct Cursor {
    pub town: TownName,
    pub catalog: Catalog,
}

/// Set a named persistent context block, or
/// clear it with None. Its implementation owns compaction and tool-time notices.
pub(crate) trait ContextBlocks {
    fn set(&mut self, resident: &ResidentId, name: &str, text: Option<&str>, facts: &[String]) -> Result<(), Failure>;
}

pub(crate) async fn text(
    node: &Arc<Node>,
    town: &TownName,
    acting: &Acting,
    read_only: bool,
) -> Result<Option<String>, Failure> {
    if !super::settings::enabled_for(node, acting, town)? {
        return Ok(None);
    }
    let catalog = super::api::catalog_for(node, acting, town).await?;
    let folders = super::folders::get(node, town, acting).await;
    Ok(Some(fastctx_hands::memory::map(&catalog, folders.as_ref().map_err(String::as_str), read_only)))
}

/// Refresh after project membership, memory, account settings, or online machines
/// change. Keep the returned cursor only after the sink succeeds. Initial
/// registration and project changes emit no memory-change notice.
pub(crate) async fn refresh(
    node: &Arc<Node>,
    id: &ResidentId,
    previous: Option<&Cursor>,
    sink: &mut impl ContextBlocks,
) -> Result<Option<Cursor>, Failure> {
    let (about, town) = node.residents.about(id).ok_or_else(|| Failure::refused("unknown conversation"))?;
    let me = Address { machine: node.machine(), resident: id.clone() };
    let Some(town) = town else { return hand(&me, None, previous, sink) };
    crate::town::access::ready(node, &about.acting, &town).await?;
    if !super::settings::enabled_for(node, &about.acting, &town)? {
        return hand(&me, None, previous, sink);
    }
    let catalog = super::api::catalog_for(node, &about.acting, &town).await?;
    let folders = super::folders::get(node, &town, &about.acting).await;
    let attributes = crate::town::instructions::text(node, &town, &about.acting)
        .map_err(|error| Failure::new(error.to_string()))?
        .ok_or_else(|| Failure::new("the project record is unavailable"))?;
    hand(
        &me,
        Some(Snapshot {
            town,
            attributes,
            catalog,
            folders,
            read_only: matches!(about.origin, Origin::Colleague { .. }),
        }),
        previous,
        sink,
    )
}

struct Snapshot {
    town: TownName,
    attributes: String,
    catalog: Catalog,
    folders: Result<Folders, String>,
    read_only: bool,
}

fn hand(
    me: &Address,
    snapshot: Option<Snapshot>,
    previous: Option<&Cursor>,
    sink: &mut impl ContextBlocks,
) -> Result<Option<Cursor>, Failure> {
    let Some(snapshot) = snapshot else {
        sink.set(&me.resident, BLOCK, None, &[])?;
        return Ok(None);
    };
    let Snapshot { town, attributes, catalog, folders, read_only } = snapshot;
    let memory = fastctx_hands::memory::map(&catalog, folders.as_ref().map_err(String::as_str), read_only);
    let facts = previous
        .filter(|before| before.town == town)
        .map(|before| changes(&before.catalog, &catalog, me))
        .unwrap_or_default();
    sink.set(&me.resident, BLOCK, Some(&format!("{attributes}\n{memory}")), &facts)?;
    Ok(Some(Cursor { town, catalog }))
}

fn changes(before: &Catalog, after: &Catalog, me: &Address) -> Vec<String> {
    let versions: std::collections::BTreeMap<_, _> =
        before.journals.iter().chain(&before.specs).map(|revision| (&revision.name, revision.version)).collect();
    after.journals.iter().chain(&after.specs)
        .filter(|revision| versions.get(&revision.name) != Some(&revision.version))
        .filter(|revision| !matches!(&revision.author.stamp.by, fastctx_types::project::Writer::Resident { address } if address == me))
        .map(fastctx_hands::memory::changed).collect()
}
