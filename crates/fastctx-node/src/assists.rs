//! A session on a login assisting another account works for the accounts
//! it assists (`world-network.md`, 共享机器与智能体协助), and takes a new
//! turn only while one of them may still use that login here, within the
//! allowances the assistance gives it (`usage`). When the assistance is
//! ended or reaches its deadline, the turn already running goes on, which
//! the owner sees and may stop (收回立刻生效); nothing starts another:
//! words, a colleague's answer or a queued message for it stay kept, and go
//! in with whatever is next handed to it once the login assists them again,
//! or their allowance has room again.

use fastctx_types::access::Acting;
use fastctx_types::ipc::Failure;
use fastctx_types::residents::About;

use crate::Node;

pub(crate) mod host;

/// Whether a session here working for `acting` assists another account
/// with one of this computer's logins: the accounts it works for may not
/// work here themselves.
pub(crate) fn assisting(node: &Node, acting: &Acting) -> bool {
    node.works_here(acting).is_err()
}

/// Whether the session `about` may take a new turn now. One that works for
/// accounts needs one of them logged in here or allowed to use its login
/// here, as this machine's view of their hub says at this moment; an
/// account of a hub that has given this machine no view yet, or that this
/// machine is no longer on, has nothing to judge by and is passed over.
pub(crate) fn may_turn(node: &Node, id: &fastctx_types::ResidentId, about: &About) -> Result<(), Failure> {
    let Acting::Accounts { accounts } = &about.acting else { return Ok(()) };
    let options = crate::session_options::read(node, id)?;
    let model = options.model.as_deref().or(about.model.as_deref());
    let mut refused = Vec::new();
    let mut spent = None;
    for account in accounts {
        let Some(link) = node.link_of(&account.hub) else { continue };
        let view = link.view();
        let Some(here) = view.me().map(|me| me.name.clone()) else { continue };
        if view.logged_in(&here, &account.account) {
            return Ok(());
        }
        if view.may_use_login(&account.account, &here, &about.host.agent, about.home.as_deref()) {
            if assisting(node, &about.acting) {
                host::require(node, id)?;
            }
            match crate::usage::used_up(node, about, account, model) {
                None => return Ok(()),
                Some(used) => {
                    spent.get_or_insert(used.said(account));
                    continue;
                }
            }
        }
        refused.push(account.account.to_string());
    }
    if let Some(said) = spent {
        return Err(Failure::not_run(said));
    }
    if refused.is_empty() {
        return Ok(());
    }
    Err(Failure::not_run(format!(
        "the {} login this conversation runs on no longer assists {}; what is for it waits until it assists them again",
        about.host.agent,
        refused.join(", ")
    )))
}
