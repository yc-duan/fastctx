//! What a session working in a project is given of it (`world-projects.md`,
//! 须知): the project's record, the user's instructions for all work in it
//! first. A session the World starts has it in the instructions it starts
//! with. A Claude session also has it beside its files, which its plugin
//! puts into its context each time Claude Code reads that again, as after
//! compacting; it is kept up to date as the record changes, and as the
//! session's resident moves between projects. Every session is told the
//! record again when it changes (`notes`).

use std::io;
use std::path::Path;

use fastctx_types::access::Acting;
use fastctx_types::project::Stamp;
use fastctx_types::{ResidentId, TownName};

use crate::Node;

/// The record of `town` as a session working for `acting` reads it, from
/// this machine's copy of its ledger, naming only the machines it may work
/// on; none when this machine has no record of it.
pub(crate) fn text(node: &Node, town: &TownName, acting: &Acting) -> io::Result<Option<String>> {
    let records = super::read::records(&node.world, town)?;
    let (Some(first), Some(record)) = (records.first(), records.last()) else { return Ok(None) };
    let opened = Stamp { by: first.by.clone(), on: first.on.clone(), at: first.at };
    let mut record = record.clone();
    crate::ledger::query::seen(node, acting, town, &mut record)?;
    Ok(Some(fastctx_hands::town::record_of(town, &record, &opened)))
}

/// Puts the record of the project the resident `id` works in beside the
/// files of its Claude session in `session` (`project.md`), where its
/// plugin reads it; takes it away when it works in none.
pub(crate) fn hand_to_session(node: &Node, id: &ResidentId, session: &Path) -> io::Result<()> {
    let target = session.join("project.md");
    let text = match node.residents.town_of(id).flatten() {
        Some(town) => text(node, &town, &acting(node, id))?,
        None => None,
    };
    match text {
        Some(text) => match std::fs::read(&target) {
            Ok(held) if held == text.as_bytes() => Ok(()),
            _ => fastctx_platform::publish(&target, text.as_bytes(), fastctx_platform::Permissions::Private),
        },
        None => match std::fs::remove_file(&target) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        },
    }
}

/// Whom the resident `id` works for.
pub(crate) fn acting(node: &Node, id: &ResidentId) -> Acting {
    node.residents.about(id).map(|(about, _)| about.acting).unwrap_or_default()
}

/// Hands the record again to the Claude session of the resident `id`, as
/// after it moved; what goes wrong is logged.
pub(crate) fn hand_to_current(node: &Node, id: &ResidentId) {
    let Some((about, _)) = node.residents.about(id) else { return };
    let Some(session) = about.session.filter(|_| about.host.agent.as_str() == "claude") else { return };
    let directory = fastctx_hosts::claude::exchange::sessions(node.world.path()).join(session);
    if directory.is_dir()
        && let Err(error) = hand_to_session(node, id, &directory)
    {
        node.log.line(format_args!("handing {id}'s project to its Claude session: {error}"));
    }
}

/// Hands the record of `town` again to the Claude session of every
/// resident working in it here, after its ledger changed.
pub(crate) fn refresh(node: &Node, town: &TownName) {
    for resident in node.residents.everyone() {
        if resident.removed.is_none() && resident.town.as_ref() == Some(town) {
            hand_to_current(node, &resident.id);
        }
    }
}
