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

use fastctx_types::{ResidentId, TownName};

use crate::Node;

/// The record of `town` as a session working for `acting` reads it, from
/// this machine's copy of its ledger, naming only the machines it may work
/// on; none when this machine has no record of it.
pub(crate) fn current(
    node: &Node,
    town: &TownName,
    acting: &Acting,
) -> io::Result<Option<(String, fastctx_types::project::Record)>> {
    if !super::access::permits(node, acting, town).map_err(io::Error::other)? {
        return Ok(None);
    }
    let Some((mut record, opened)) = super::read::current(&node.world, town)? else { return Ok(None) };
    crate::ledger::query::seen(node, acting, town, &mut record)?;
    Ok(Some((fastctx_hands::town::record_of(town, &record, &opened), record)))
}

/// Refreshes named project context and gives the plugin its canonical location.
/// Leaving a project clears the block through the same replacement path.
pub(crate) fn hand_to_session(node: &Node, id: &ResidentId, session: &Path) -> io::Result<()> {
    crate::context::refresh_local(node, id)?;
    crate::context::hand_to_session(node, id, session)
}

/// Whom the resident `id` works for.
pub(crate) fn acting(node: &Node, id: &ResidentId) -> Acting {
    node.residents.about(id).map(|(about, _)| about.acting).unwrap_or_default()
}

/// Hands the record again to the Claude session of the resident `id`, as
/// after it moved. A failed write is returned to the operation that moved it.
pub(crate) fn hand_to_current(node: &Node, id: &ResidentId) -> io::Result<()> {
    let Some((about, _)) = node.residents.about(id) else { return Ok(()) };
    crate::context::refresh_local(node, id)?;
    let Some(session) = about.session.filter(|_| about.host.agent.as_str() == "claude") else { return Ok(()) };
    let directory = fastctx_hosts::claude::exchange::sessions(node.world.path()).join(session);
    if directory.is_dir() {
        hand_to_session(node, id, &directory)?;
    }
    Ok(())
}

/// Hands the record of `town` again to the Claude session of every
/// resident working in it here, after its ledger changed.
pub(crate) fn refresh(node: &Node, town: &TownName) {
    for resident in node.residents.everyone() {
        if resident.removed.is_none()
            && resident.town.as_ref() == Some(town)
            && let Err(error) = hand_to_current(node, &resident.id)
        {
            node.log.line(format_args!("refreshing {}'s project instructions: {error}", resident.id));
        }
    }
}
