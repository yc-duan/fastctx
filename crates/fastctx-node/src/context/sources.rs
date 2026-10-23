//! Adapters from the user's settings and project records into named context.

use super::{Change, Receipt, storage, words};
use crate::Node;
use fastctx_types::{Address, ResidentId, TownName, access::Acting, ipc::Failure, project::Writer, residents::Origin};
use std::{io, sync::Arc};

pub(crate) fn refresh_local(node: &Node, id: &ResidentId) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    let (about, town) = node.residents.about(id).ok_or_else(|| io::Error::other(words::UNKNOWN))?;
    local_locked(node, id, town.as_ref(), &about.acting)
}

fn local(node: &Node, id: &ResidentId, town: Option<&TownName>, acting: &Acting) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    local_locked(node, id, town, acting)
}

fn local_locked(node: &Node, id: &ResidentId, town: Option<&TownName>, acting: &Acting) -> io::Result<()> {
    let prompt = crate::profile::session::text(&crate::profile::session::prompt(node, acting));
    storage::set_locked(
        &node.world,
        id,
        "user",
        &prompt,
        Change { by: None, version: "", fact: &words::changed("user", words::USER), full: true },
    )?;
    let (text, author, version, by) = match town {
        Some(town) => match crate::town::instructions::current(node, town, acting)? {
            Some((text, record)) => {
                let version = format!("{town}:{}", record.version);
                let writer = fastctx_hands::town::writer(&record.by, &record.on);
                let by = match record.by {
                    Writer::Resident { address } => Some(address),
                    _ => None,
                };
                (text, by, version, writer)
            }
            None => (String::new(), None, String::new(), words::USER.into()),
        },
        None => (String::new(), None, String::new(), words::USER.into()),
    };
    storage::set_locked(
        &node.world,
        id,
        "project",
        &text,
        Change { by: author.as_ref(), version: &version, fact: &words::changed("project", &by), full: true },
    )?;
    let focus = match std::fs::read_to_string(node.world.conversation(id).focus()) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    storage::set_locked(
        &node.world,
        id,
        "focus",
        &focus,
        Change { by: None, version: "selection", fact: &words::changed("focus", words::USER), full: true },
    )
}

pub(crate) async fn prepare(
    node: &Arc<Node>,
    id: &ResidentId,
    town: Option<&TownName>,
    origin: &Origin,
    acting: &Acting,
) -> Result<(String, Receipt), Failure> {
    let directory = node.world.conversation(id);
    fastctx_platform::ensure_private_dir(directory.path()).map_err(|error| Failure::new(error.to_string()))?;
    if let Some(town) = town {
        crate::town::access::ready(node, acting, town).await?;
    }
    if node.residents.about(id).is_some() {
        crate::focus::refresh(node, id).await?;
    }
    local(node, id, town, acting).map_err(|error| Failure::new(error.to_string()))?;
    crate::plan::context(node, id, origin, acting).await?;
    storage::opening(&node.world, id).map_err(|error| Failure::new(error.to_string()))
}

pub(super) fn observed(node: &Node, id: &ResidentId, receipt: &Receipt) -> io::Result<()> {
    if let Some(version) = receipt.versions.get("plan").filter(|version| !version.is_empty()) {
        crate::plan::context_seen(node, id, version).map_err(|error| io::Error::other(error.message))?;
    }
    Ok(())
}

pub(crate) fn plan(node: &Node, id: &ResidentId, page: &fastctx_types::plan::Page, version: &str) -> io::Result<()> {
    let by: Option<&Address> = page.body.as_ref().and_then(|body| match &body.author {
        fastctx_types::plan::Author::Resident { address } => Some(address),
        fastctx_types::plan::Author::User => None,
    });
    let who = by.map(|by| by.resident.to_string()).unwrap_or_else(|| words::USER.into());
    let set = |text: &str, change| {
        if node.residents.about(id).is_some() {
            super::set(node, id, "plan", text, change)
        } else {
            storage::set(&node.world, id, "plan", text, change)
        }
    };
    set(
        page.body.as_ref().map(|body| body.markdown.as_str()).unwrap_or_default(),
        Change { by, version, fact: &words::changed("plan", &who), full: true },
    )
}

pub(crate) fn plan_unavailable(node: &Node, id: &ResidentId) -> io::Result<()> {
    storage::set(
        &node.world,
        id,
        "plan",
        "",
        Change { by: None, version: "", fact: words::PLAN_UNAVAILABLE, full: true },
    )
}
