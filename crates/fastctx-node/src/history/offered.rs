//! Where history is offered (`world-history.md`): in a build that offers it
//! (`fastctx_platform::history`), to the conversations of the logins the
//! workbench is open in (`world-layers.md`, 工作台：在一个登录上打开).

use crate::{Node, town::caller::Caller};
use fastctx_types::{ipc::Failure, residents::About};

/// Said where this build does not offer history.
const UNOFFERED: &str = "history is not enabled in this release";
/// Said to a conversation of a login the workbench is not open in.
const CLOSED: &str = "history is part of the workbench; open the workbench in this login to use it";

/// Whether the conversation `about` is offered history.
pub(crate) fn to(node: &Node, about: &About) -> bool {
    fastctx_platform::history()
        && crate::logins::workbench::is_open(node, &about.acting, about.host.agent.as_str(), about.home.as_deref())
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
