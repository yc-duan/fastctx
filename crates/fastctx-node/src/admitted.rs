//! Who came into an account this computer is logged in to (`world-network.md`,
//! 登录与入网): a device that logged in to it, or a machine registered
//! under it. Each time a hub's directory changes, a member of one of these
//! accounts that this computer has not seen there before is noted, with
//! when it was noticed, until the user has seen it, so the interface can
//! say who came in and offer 「不是我」, which removes it. The directory, as
//! the hub pushes it, is the only source.
//!
//! ```text
//! world/net/<hub id>/arrivals.json   who this computer has seen in its accounts there, and who it has not shown yet
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use fastctx_net::sign::PublicKey;
use fastctx_types::access::HubId;
use fastctx_types::accounts::Arrival;
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Node;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct File {
    /// Every member of each account that this computer has noticed.
    seen: BTreeMap<AccountName, BTreeSet<(MachineName, PublicKey)>>,
    /// Those the user has not been shown yet.
    fresh: Vec<Arrival>,
}

fn path(node: &Node, hub: &HubId) -> PathBuf {
    crate::world::hub_dir(&node.world, hub).join("arrivals.json")
}

/// Notes who came into the accounts this computer is logged in to at `hub`
/// that it has not seen there before. The first time it looks at an
/// account, everyone there counts as seen.
pub fn notice(node: &Node, hub: &HubId) {
    let Some(link) = node.link_of(hub) else { return };
    let view = link.view();
    let Some(me) = view.me() else { return };
    let path = path(node, hub);
    let mut file: File = match fastctx_net::link::state::read_or(&path, File::default()) {
        Ok(file) => file,
        Err(error) => {
            node.log.line(format_args!("reading who came into this computer's accounts: {error}"));
            return;
        }
    };
    let mut changed = false;
    for account in &me.logins {
        let members: BTreeSet<(MachineName, PublicKey)> = view
            .members
            .iter()
            .filter(|member| member.key != me.key)
            .filter(|member| member.logins.contains(account) || member.home.as_ref() == Some(account))
            .map(|member| (member.name.clone(), member.key))
            .collect();
        let first = !file.seen.contains_key(account);
        let seen = file.seen.entry(account.clone()).or_default();
        for (name, key) in members {
            if seen.insert((name.clone(), key)) {
                changed = true;
                if !first {
                    let device = view.member(&name).is_some_and(|member| !member.is_machine());
                    node.log.line(format_args!("{name} came into the account {account} of the hub {}", view.hub_name));
                    file.fresh.push(Arrival {
                        hub: hub.clone(),
                        account: account.clone(),
                        name,
                        device,
                        noticed_at: Timestamp::now(),
                    });
                }
            }
        }
        changed |= first;
    }
    if changed && let Err(error) = fastctx_net::link::state::write(&path, &file) {
        node.log.line(format_args!("recording who came into this computer's accounts: {error}"));
    }
}

/// Who came into this computer's accounts at `hub` and was not shown yet.
pub fn fresh(node: &Node, hub: &HubId) -> Vec<Arrival> {
    fastctx_net::link::state::read_or(&path(node, hub), File::default()).map(|file| file.fresh).unwrap_or_default()
}

/// The user has seen that `name` came into `account` at `hub`.
pub fn shown(node: &Node, hub: &HubId, account: &AccountName, name: &MachineName) -> std::io::Result<()> {
    let path = path(node, hub);
    let mut file: File = fastctx_net::link::state::read_or(&path, File::default())?;
    file.fresh.retain(|arrival| !(arrival.account == *account && arrival.name == *name));
    fastctx_net::link::state::write(&path, &file)
}
