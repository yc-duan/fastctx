//! History requires the build's support, the acting account's opt-in, and
//! an open workbench in the conversation's login.

use crate::{Node, town::caller::Caller};
use fastctx_types::{ipc::Failure, residents::About};

/// Said where this build does not offer history.
const UNOFFERED: &str = "history is not enabled in this release";
/// Said when the account or the login does not offer history.
const CLOSED: &str =
    "history is off for this conversation; enable history for its account and open the workbench in this login";

/// Whether the conversation `about` is offered history.
pub(crate) fn to(node: &Node, about: &About) -> bool {
    let enabled = if node.connected_anywhere() {
        node.links()
            .iter()
            .any(|link| node.accounts_on(link, &about.acting).iter().any(|account| super::switch::on(link, account)))
    } else {
        super::switch::alone(node)
    };
    enabled && crate::logins::workbench::is_open(node, &about.acting, about.host.agent.as_str(), about.home.as_deref())
}

/// Refuses `caller` history where it is not offered. A conversation of
/// another machine was offered it there, where its login is known.
pub(crate) fn check(node: &Node, caller: &Caller) -> Result<(), Failure> {
    if !fastctx_platform::history() {
        return Err(Failure::not_run(UNOFFERED));
    }
    match caller {
        Caller::Resident(address) if node.is_me(&address.machine) => match node.residents.about(&address.resident) {
            Some((about, _)) if to(node, &about) => Ok(()),
            _ => Err(Failure::refused(CLOSED)),
        },
        _ => Ok(()),
    }
}
