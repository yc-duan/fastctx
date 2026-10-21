//! Native fork history can keep a deleted conversation's host record alive.

use std::io;

use fastctx_types::layout::{ConversationDir, Kind};
use fastctx_types::residents::About;
use fastctx_types::session::{Happened, SessionChange};
use fastctx_types::{Address, ResidentId};

use crate::Node;

pub(crate) const RETAINED_HOST: &str = "retained-host.json";

pub(crate) fn retained(directory: &ConversationDir) -> io::Result<Option<About>> {
    match std::fs::read(directory.path().join(RETAINED_HOST)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn source(directory: &ConversationDir, about: &About) -> io::Result<Option<Address>> {
    if let Some(source) = &about.forked_from {
        return Ok(Some(source.clone()));
    }
    // Older forks recorded their origin in the durable event stream only.
    Ok(crate::delivery::history(directory)?.into_iter().rev().find_map(|event| match event.happened {
        Happened::Changed { change: SessionChange::Forked { source, .. }, .. } => Some(source),
        _ => None,
    }))
}

fn about(directory: &ConversationDir) -> io::Result<Option<About>> {
    match crate::residents::read_about(directory) {
        Ok(about) => Ok(Some(about)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => retained(directory),
        Err(error) => Err(error),
    }
}

/// An archived fork still holds history. A logically deleted fork holds it until its own dependents end.
pub(crate) fn has_dependents(node: &Node, id: &ResidentId) -> io::Result<bool> {
    for kept in node.world.kept()? {
        let Kind::Home(child) = &kept.kind else { continue };
        if child == id {
            continue;
        }
        let Some(about) = about(&kept.directory)? else { continue };
        if about.host.agent.as_str() == "codex"
            && source(&kept.directory, &about)?
                .is_some_and(|source| node.is_me(&source.machine) && source.resident == *id)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn keep(node: &Node, about: &About) -> io::Result<()> {
    let directory = node.world.conversation(&about.id);
    let mut about = about.clone();
    about.forked_from = source(&directory, &about)?;
    fastctx_platform::publish(
        &directory.path().join(RETAINED_HOST),
        &serde_json::to_vec(&about)?,
        fastctx_platform::Permissions::Private,
    )
}

/// A deleted native parent to revisit after its child has been physically removed.
pub(crate) fn parent(node: &Node, id: &ResidentId) -> io::Result<Option<Address>> {
    let directory = node.world.conversation(id);
    let Some(about) = about(&directory)? else { return Ok(None) };
    let Some(source) = source(&directory, &about)? else { return Ok(None) };
    if node.is_me(&source.machine) && retained(&node.world.conversation(&source.resident))?.is_some() {
        Ok(Some(source))
    } else {
        Ok(None)
    }
}
