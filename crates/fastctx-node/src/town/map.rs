//! Project attributes and the optional generated Markdown-memory index.

use crate::Node;
use fastctx_types::{TownName, access::Acting, ipc::Failure, project::MapView};
use std::sync::Arc;

pub async fn view(node: &Arc<Node>, town: &TownName, acting: &Acting) -> Result<MapView, Failure> {
    view_for(node, town, acting, false).await
}

pub(crate) async fn view_for(
    node: &Arc<Node>,
    town: &TownName,
    acting: &Acting,
    read_only: bool,
) -> Result<MapView, Failure> {
    super::access::ready(node, acting, town).await?;
    let (world, name) = (node.world.clone(), town.clone());
    let (records, pages) = tokio::task::spawn_blocking(move || {
        let pages = if !world.town(&name).signed().is_dir() {
            crate::ledger::metadata::read(&world, &name)?.map(|index| index.pages).unwrap_or_default()
        } else {
            crate::pages::read::summaries(&world, &name)?
        };
        std::io::Result::Ok((super::read::current(&world, &name)?, pages))
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?
    .map_err(|error| Failure::new(error.to_string()))?;
    let Some((mut record, mut opened)) = records else { return Err(no_ledger(node, town)) };
    let mut pages = pages;
    crate::ledger::query::seen(node, acting, town, &mut record).map_err(|error| Failure::new(error.to_string()))?;
    crate::ledger::query::seen_stamp(node, acting, town, &mut opened)
        .map_err(|error| Failure::new(error.to_string()))?;
    if let Some(usable) =
        crate::ledger::query::usable(node, acting, town).map_err(|error| Failure::new(error.to_string()))?
    {
        pages.iter_mut().for_each(|summary| summary.current.for_reader(&*usable));
    }
    let memory = crate::memory::context::text(node, town, acting, read_only).await?.map(|(text, _)| text);
    Ok(MapView {
        memory,
        town: town.clone(),
        ledger: String::new(),
        record,
        opened,
        journals: Vec::new(),
        pages,
        canon: None,
    })
}

pub async fn brief(node: &Arc<Node>, town: &TownName, acting: &Acting) -> Result<String, Failure> {
    brief_for(node, town, acting, false).await
}

pub(crate) async fn brief_for(
    node: &Arc<Node>,
    town: &TownName,
    acting: &Acting,
    read_only: bool,
) -> Result<String, Failure> {
    Ok(fastctx_hands::town::map(&view_for(node, town, acting, read_only).await?))
}

pub async fn query(node: &Arc<Node>, town: &TownName) -> Result<(), Failure> {
    let towns = crate::ledger::query(node, Some(town)).await.map_err(Failure::new)?;
    if towns.contains(town) { Ok(()) } else { Err(no_ledger(node, town)) }
}

pub fn no_ledger(_node: &Node, town: &TownName) -> Failure {
    Failure::new(format!("no project record is available for {town}"))
}
