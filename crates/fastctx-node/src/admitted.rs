//! Who came in with this machine's invitations (`world-network.md`,
//! 邀请码): each time the members change, a member this machine invited and
//! has not seen before is noted, with when it was noticed, until the user
//! has seen it, so the interface can say who joined and offer to revoke
//! it. The members table, as this machine checks it, is the only source:
//! nothing the hub says on its own counts.
//!
//! ```text
//! world/net/admitted.json   the members this machine invited and has seen, and those not yet shown
//! ```

use std::collections::BTreeSet;
use std::path::PathBuf;

use fastctx_net::trust::Kind;
use fastctx_types::MachineName;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Node;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct File {
    /// Every member this machine invited that it has noticed.
    seen: BTreeSet<MachineName>,
    /// Those the user has not been shown yet.
    fresh: Vec<Admitted>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Admitted {
    pub name: MachineName,
    pub device: bool,
    pub noticed_at: Timestamp,
}

fn path(node: &Node) -> PathBuf {
    node.world.net().join("admitted.json")
}

/// Notes the members invited by this machine that it has not seen before.
/// The first time, every member there is counts as seen.
pub fn notice(node: &Node) {
    let Some(link) = node.link() else { return };
    let (table, _) = link.members();
    let me = node.machine();
    let mine: Vec<(MachineName, Kind)> = table
        .members()
        .filter(|member| member.invited_by.as_ref() == Some(&me))
        .map(|member| (member.name.clone(), member.kind))
        .collect();
    let path = path(node);
    let first = !path.exists();
    let mut file: File = match fastctx_net::link::state::read_or(&path, File::default()) {
        Ok(file) => file,
        Err(error) => {
            node.log.line(format_args!("reading who came in with this machine's invitations: {error}"));
            return;
        }
    };
    let mut changed = first;
    for (name, kind) in mine {
        if file.seen.insert(name.clone()) {
            changed = true;
            if !first {
                node.log.line(format_args!("{name} joined the World with an invitation from this machine"));
                file.fresh.push(Admitted { name, device: !kind.is_machine(), noticed_at: Timestamp::now() });
            }
        }
    }
    if changed && let Err(error) = fastctx_net::link::state::write(&path, &file) {
        node.log.line(format_args!("recording who came in with this machine's invitations: {error}"));
    }
}

/// Those who joined with this machine's invitations and were not shown yet.
pub fn fresh(node: &Node) -> Vec<Admitted> {
    fastctx_net::link::state::read_or(&path(node), File::default()).map(|file| file.fresh).unwrap_or_default()
}

/// The user has seen that `name` joined.
pub fn shown(node: &Node, name: &MachineName) -> std::io::Result<()> {
    let path = path(node);
    let mut file: File = fastctx_net::link::state::read_or(&path, File::default())?;
    file.fresh.retain(|admitted| admitted.name != *name);
    fastctx_net::link::state::write(&path, &file)
}
