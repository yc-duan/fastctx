//! Who came into an account this computer is logged in to
//! (`fastctx_net::arrivals`), kept for each hub beside its other records.
//!
//! ```text
//! world/net/<hub id>/arrivals.json   who this computer has seen in its accounts there, and who it has not shown yet
//! ```

use std::path::PathBuf;

use fastctx_types::access::HubId;
use fastctx_types::accounts::Arrival;
use fastctx_types::{AccountName, MachineName};

use crate::Node;

fn path(node: &Node, hub: &HubId) -> PathBuf {
    crate::world::hub_dir(&node.world, hub).join("arrivals.json")
}

/// Notes who came into the accounts this computer is logged in to at `hub`.
pub fn notice(node: &Node, hub: &HubId) {
    let Some(link) = node.link_of(hub) else { return };
    let view = link.view();
    match fastctx_net::arrivals::notice(&path(node, hub), &view) {
        Ok(noticed) => {
            for arrival in noticed {
                node.log.line(format_args!(
                    "{} came into the account {} of the hub {}",
                    arrival.name, arrival.account, view.hub_name
                ));
            }
        }
        Err(error) => node.log.line(format_args!("keeping who came into this computer's accounts: {error}")),
    }
}

/// Who came into this computer's accounts at `hub` and was not shown yet.
pub fn fresh(node: &Node, hub: &HubId) -> Vec<Arrival> {
    fastctx_net::arrivals::fresh(&path(node, hub)).unwrap_or_else(|error| {
        node.log.line(format_args!("reading who came into this computer's accounts: {error}"));
        Vec::new()
    })
}

/// The user has seen that `name` came into `account` at `hub`.
pub fn shown(node: &Node, hub: &HubId, account: &AccountName, name: &MachineName) -> std::io::Result<()> {
    fastctx_net::arrivals::shown(&path(node, hub), account, name)
}
