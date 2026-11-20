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
/// work here themselves. While a hub has not given this connection its
/// view yet, it is taken to, which keeps the owner's own (their
/// instructions, their memory) from it for the moment; what is recorded or
/// routed by it asks `assisting_known`.
pub(crate) fn assisting(node: &Node, acting: &Acting) -> bool {
    node.works_here(acting).is_err()
}

/// Whether a session for `acting` assists another account, as its hubs now
/// say; an error while one has not given this connection its view yet.
pub(crate) fn assisting_known(node: &Node, acting: &Acting) -> Result<bool, Failure> {
    node.may_work_here(acting).map(|works| !works)
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

/// Whom a session the World starts here for `acting`, in `agent`'s login
/// `home`, is born working for, and by what right (`About::born_as`), as
/// this machine's hubs say now: each account, which account of its name it
/// is, and the assistance it works by where it does not hold the machine
/// (design-forget-account-v2.md F0). The account that owns this computer is
/// read as its hub last said while the hub has not said again, as a session
/// working for it is let work here meanwhile (`authorization`), so its own
/// work is not held up; another whose hub has not said yet, or that may no
/// longer use the login here, is refused: nothing is born for it, as a call
/// from it would be refused (`admission`). The assistance is the one the hub
/// gave: one from a hub from before grants has none, worked by all the same.
pub(crate) fn born_as(
    node: &Node,
    acting: &Acting,
    agent: &fastctx_types::AgentName,
    home: Option<&std::path::Path>,
) -> Result<Vec<fastctx_types::residents::BornAs>, Failure> {
    let Acting::Accounts { accounts } = acting else { return Ok(Vec::new()) };
    accounts
        .iter()
        .map(|account| {
            let link = node.link_of(&account.hub).ok_or_else(|| {
                Failure::from(fastctx_types::authorization::Denial::new(
                    fastctx_types::authorization::DenialReason::ViewUnavailable,
                    "the account it works for is no longer connected; it waits for authority",
                ))
            })?;
            let view = match crate::authorization::current_view(&link) {
                Ok(view) => view,
                Err(_) if link.view().me().is_some_and(|me| me.home.as_ref() == Some(&account.account)) => link.view(),
                Err(failure) => return Err(failure),
            };
            born_by(node, &view, &account.hub, &account.account, agent, home)
        })
        .collect()
}

/// What `account` of `hub` is born as here in `agent`'s login `home`, as
/// `view` says (`born_as`): read by the door as it lets a call to open one
/// in, from the very view it lets it in by, as by `born_as` as it is born.
pub(crate) fn born_by(
    node: &Node,
    view: &fastctx_net::directory::Directory,
    hub: &fastctx_types::access::HubId,
    account: &fastctx_types::AccountName,
    agent: &fastctx_types::AgentName,
    home: Option<&std::path::Path>,
) -> Result<fastctx_types::residents::BornAs, Failure> {
    let here = node.machine();
    let generation = view
        .account(account)
        .ok_or_else(|| Failure::refused(format!("there is no account {account} on its hub")))?
        .opened;
    let (assisted, grant) = if view.rights(account, &here).hands {
        (false, None)
    } else {
        let may = crate::homes::may_use_login(node, view, account, &here, agent, home)?;
        match crate::homes::assistance(node, view, account, &here, agent, home)? {
            Some(assist) if may => (true, assist.grant),
            _ => {
                let login = home.map_or_else(|| "default".to_string(), |home| home.display().to_string());
                return Err(Failure::refused(format!(
                    "the {agent} login {login} on {here} does not assist the account {account}"
                )));
            }
        }
    };
    Ok(fastctx_types::residents::BornAs { hub: hub.clone(), account: account.clone(), generation, assisted, grant })
}

/// That a session about to be started or forked here is still born by the
/// right it was born by, `born_as`, read again as late as can be before the
/// host is asked anything (codex-87c8bbd9): an error otherwise, the denial
/// of a hub that has not said again where that is why.
pub(crate) fn still_born_as(
    node: &Node,
    acting: &Acting,
    agent: &fastctx_types::AgentName,
    home: Option<&std::path::Path>,
    born: &[fastctx_types::residents::BornAs],
) -> Result<(), Failure> {
    if born_as(node, acting, agent, home)? != born {
        return Err(Failure::refused(
            "the right it was opened by no longer stands: the assistance or the account it was opened under has changed since",
        ));
    }
    Ok(())
}

/// A session born working by an assistance is born in a login it knows by
/// its id, or not at all: what it was born under is what ending the
/// assistance takes away (design-forget-account-v2.md F0, codex-d1962125).
pub(crate) fn evidenced(
    born_as: &[fastctx_types::residents::BornAs],
    login: Option<&fastctx_types::logins::LoginId>,
) -> Result<(), Failure> {
    if login.is_none() && born_as.iter().any(|born| born.assisted) {
        return Err(Failure::new(
            "the login it would be born in could not be given its id (the node's log says why), and a session working by an assistance is born only in a login known by its id",
        ));
    }
    Ok(())
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
pub(crate) async fn may_turn(
    node: &std::sync::Arc<Node>,
    id: &fastctx_types::ResidentId,
    about: &About,
) -> Result<(), Failure> {
    admit_turn(node, id, about).await.map_err(|error| error.failure)
}

pub(crate) async fn admit_turn(
    node: &std::sync::Arc<Node>,
    id: &fastctx_types::ResidentId,
    about: &About,
) -> Result<(), TurnRefusal> {
    let (node, id, about) = (node.clone(), id.clone(), about.clone());
    fastctx_platform::task::spawn_blocking(move || check_turn(&node, &id, &about))
        .await
        .map_err(|error| TurnRefusal::from(Failure::new(error.to_string())))?
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
    let Acting::Accounts { accounts } = &about.acting else {
        // The machine's own turns are held by the login's Fable line alone.
        if let Some(line) = crate::usage::fable_line(node, about) {
            let options = crate::session_options::read(node, id)?;
            if line.applies(options.model.as_deref().or(about.model.as_deref())) {
                return Err(Failure::not_run(line.said(None)).into());
            }
        }
        return Ok(());
    };
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
        if assisting_known(node, &about.acting)? && host::assisted(node, id)? != Some(true) {
            return Err(TurnRefusal { failure: host::legacy(), permanent: true });
        }
        match crate::usage::used_up(node, about, account, model) {
            None => return Ok(()),
            Some(used) => {
                spent.get_or_insert(used.said(Some(account)));
            }
        }
    }
    if let Some(said) = spent {
        return Err(Failure::not_run(said).into());
    }
    Err(refused.unwrap_or_else(crate::authorization::no_subject).into())
}
