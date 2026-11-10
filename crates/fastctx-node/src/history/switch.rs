//! Account opt-in and the user-facing switch operations.

use crate::Node;
use fastctx_net::link::Link;
use fastctx_types::{AccountName, HubName, access::HubAccount, history_switch::HistorySwitch, ipc::Failure};
use std::sync::Arc;

pub(crate) const COPIES_OFF: &str = "notes synchronization is off on this computer";

pub(crate) fn on(link: &Link, account: &AccountName) -> bool {
    let view = link.view();
    fastctx_platform::history()
        && view.history_switch
        && view.account(account).is_some_and(|found| !found.locked && found.history)
}

pub(crate) fn enabled(node: &Node, account: &HubAccount) -> bool {
    node.link_of(&account.hub).is_some_and(|link| on(&link, &account.account))
}

pub(crate) fn alone(node: &Node) -> bool {
    fastctx_platform::history()
        && !node.connected_anywhere()
        && super::own_switch::read(node).is_ok_and(|setting| setting.on)
}

fn own(node: &Node, hub: Option<&HubName>) -> Result<(), Failure> {
    if hub.is_some() || node.connected_anywhere() {
        return Err(Failure::not_run("name the account whose notes setting is being changed"));
    }
    Ok(())
}

pub(crate) fn read(
    node: &Node,
    hub: Option<&HubName>,
    account: Option<&AccountName>,
) -> Result<HistorySwitch, Failure> {
    match account {
        Some(account) => crate::world::hub_link(node, hub)?.history_switch_read(account).map_err(Failure::new),
        None => {
            own(node, hub)?;
            super::own_switch::read(node)
        }
    }
}

pub(crate) async fn write(
    node: &Arc<Node>,
    hub: Option<&HubName>,
    account: Option<&AccountName>,
    on: bool,
) -> Result<(), Failure> {
    match account {
        Some(account) => {
            let link = crate::world::hub_link(node, hub)?;
            link.history_switch_write(account, on).await.map_err(Failure::new)?;
            if on {
                super::import::schedule(node);
                crate::ledger::joined(node, &link.membership().id);
            }
            Ok(())
        }
        None => {
            own(node, hub)?;
            super::own_switch::write(node, on)
        }
    }
}
