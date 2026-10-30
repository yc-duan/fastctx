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

/// Whether a session here working for `acting` works only for this
/// computer's own logins. What a host keeps across a login's sessions, such
/// as Claude Code's memory of the folder they work in or Codex's memories,
/// is shared by every session of the login: it is kept only for these, and
/// never carried from one account's conversations into another's, as when a
/// machine shared whole runs another account's (`world-network.md`).
pub(crate) fn own_accounts(node: &Node, acting: &Acting) -> bool {
    let Acting::Accounts { accounts } = acting else { return true };
    accounts.iter().all(|account| {
        node.link_of(&account.hub)
            .is_some_and(|link| link.view().me().is_some_and(|me| me.logins.contains(&account.account)))
    })
}

/// Whether this account may still use the conversation's login, without applying usage limits.
pub(crate) fn permitted(
    node: &Node,
    about: &About,
    account: &fastctx_types::access::HubAccount,
) -> Result<(), Failure> {
    let link = node.link_of(&account.hub).ok_or_else(|| {
        Failure::from(fastctx_types::authorization::Denial::new(
            fastctx_types::authorization::DenialReason::ViewUnavailable,
            "the conversation's account is no longer connected; its words wait for authority",
        ))
    })?;
    let view = crate::authorization::current_view(&link)?;
    // The login as the directory writes it: a default directory written out
    // and one left unwritten are the same login (`crate::homes::assistance`).
    let home =
        crate::homes::assistance(node, &view, &account.account, &view.me, &about.host.agent, about.home.as_deref())?
            .map_or_else(|| about.home.clone(), |assist| assist.home.clone());
    view.authorize(
        &account.account,
        fastctx_net::authorization::Action::Login {
            machine: &view.me,
            agent: Some(&about.host.agent),
            home: home.as_deref(),
        },
    )
    .map_err(Failure::from)
}

/// Whether the session may start a turn under current account authority and usage allowances.
pub(crate) fn may_turn(node: &Node, id: &fastctx_types::ResidentId, about: &About) -> Result<(), Failure> {
    check_turn(node, id, about).map_err(|error| error.failure)
}

pub(crate) struct TurnRefusal {
    pub failure: Failure,
    pub permanent: bool,
}

impl From<Failure> for TurnRefusal {
    fn from(failure: Failure) -> Self {
        Self { failure, permanent: false }
    }
}

/// Only immutable birth properties justify abandoning an unsubmitted input.
pub(crate) fn check_turn(node: &Node, id: &fastctx_types::ResidentId, about: &About) -> Result<(), TurnRefusal> {
    let Acting::Accounts { accounts } = &about.acting else { return Ok(()) };
    if accounts.is_empty() {
        return Err(crate::authorization::no_subject().into());
    }
    let options = crate::session_options::read(node, id)?;
    let model = options.model.as_deref().or(about.model.as_deref());
    let mut refused = None;
    let mut spent = None;
    for account in accounts {
        if let Err(failure) = permitted(node, about, account) {
            refused = Some(failure);
            continue;
        }
        if assisting(node, &about.acting) && host::kept(node, id)? != Some(true) {
            return Err(TurnRefusal { failure: host::legacy(), permanent: true });
        }
        match crate::usage::used_up(node, about, account, model) {
            None => return Ok(()),
            Some(used) => {
                spent.get_or_insert(used.said(account));
            }
        }
    }
    if let Some(said) = spent {
        return Err(Failure::not_run(said).into());
    }
    Err(refused.unwrap_or_else(crate::authorization::no_subject).into())
}
