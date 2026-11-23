//! The rules this machine deletes its conversations idle long enough by
//! (`fastctx_types::conversation_retention`), one for each network it is on:
//! kept in `<world>/conversation-retention.json`, read and set by the
//! machine's owner on that network and by the accounts its agents assist
//! there, each for its own, and applied here by `expiry`. Who owns the
//! machine on one network sets and sees nothing of another's.

pub(crate) mod expiry;

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use fastctx_types::AccountName;
use fastctx_types::access::{Acting, HubAccount, HubId, Written};
use fastctx_types::conversation_retention::{AccountDays, GRACE_DAYS, RetentionAnswer, RetentionAsk, Rule, problem};
use fastctx_types::ipc::Failure;
use serde::{Deserialize, Serialize};

use crate::Node;

/// Held while the rules are read and written again.
static WRITING: Mutex<()> = Mutex::new(());

/// Each network's rule, as kept.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Rules {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    networks: Vec<NetworkRule>,
}

#[derive(Serialize, Deserialize)]
struct NetworkRule {
    hub: HubId,
    rule: Rule,
}

impl Rules {
    /// The rule of the network of `hub`: off where none is set.
    pub(crate) fn of(&self, hub: &HubId) -> Rule {
        self.networks.iter().find(|network| network.hub == *hub).map(|network| network.rule.clone()).unwrap_or_default()
    }

    /// Whether any network's rule ever deletes anything.
    pub(crate) fn on(&self) -> bool {
        self.networks.iter().any(|network| network.rule.on())
    }

    fn set(&mut self, hub: &HubId, rule: Rule) {
        self.networks.retain(|network| network.hub != *hub);
        if rule != Rule::default() {
            self.networks.push(NetworkRule { hub: hub.clone(), rule });
        }
    }
}

fn path(node: &Node) -> PathBuf {
    node.world.path().join("conversation-retention.json")
}

/// The rules as kept; all off where none are.
pub(crate) fn read(node: &Node) -> io::Result<Rules> {
    match fastctx_platform::disk::read(path(node)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Rules::default()),
        Err(error) => Err(error),
    }
}

fn write(node: &Node, rules: &Rules) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(rules).map_err(io::Error::other)?;
    fastctx_platform::publish(&path(node), &bytes, fastctx_platform::Permissions::Private)
}

/// Whether `after` deletes sooner than `before`: turned on, or fewer days.
fn sooner(before: Option<u32>, after: Option<u32>) -> bool {
    match (before, after) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(before), Some(after)) => after < before,
    }
}

/// `GRACE_DAYS` from now, or `kept` where that is later: whoever a rule
/// that came to delete sooner touches sees it coming.
fn graced(kept: Option<jiff::Timestamp>) -> Option<jiff::Timestamp> {
    let until = jiff::Timestamp::now() + jiff::SignedDuration::from_hours(GRACE_DAYS * 24);
    Some(kept.map_or(until, |kept| kept.max(until)))
}

/// Who asks of a network's rule.
pub(crate) enum Asker {
    /// The machine's owner on the network, or its user here.
    Owner,
    /// An account of the network the machine's agents assist.
    Assisted(HubAccount),
}

/// Answers `ask` of the rule of the network of `hub` for `asker`. Call on a
/// thread that may block.
pub(crate) fn asked(
    node: &Arc<Node>,
    hub: &HubId,
    asker: &Asker,
    ask: RetentionAsk,
) -> Result<RetentionAnswer, String> {
    let rules = {
        let _writing = WRITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut rules = read(node).map_err(|error| error.to_string())?;
        let mut rule = rules.of(hub);
        match (ask, asker) {
            (RetentionAsk::Read, _) => return Ok(answer(node, hub, asker, &rules)),
            (RetentionAsk::Owner { own_days, assisted_days }, Asker::Owner) => {
                if let Some(problem) = problem(own_days).or_else(|| problem(assisted_days)) {
                    return Err(problem);
                }
                if sooner(rule.own_days, own_days) || sooner(rule.assisted_days, assisted_days) {
                    rule.grace_until = graced(rule.grace_until);
                }
                rule.own_days = own_days;
                rule.assisted_days = assisted_days;
            }
            (RetentionAsk::Mine { days }, Asker::Assisted(of)) => {
                if of.hub != *hub {
                    return Err("an assisted account sets its days on its own network".into());
                }
                if let Some(problem) = problem(days) {
                    return Err(problem);
                }
                if let (Some(days), Some(ceiling)) = (days, rule.assisted_days)
                    && days > ceiling
                {
                    return Err(format!(
                        "this machine's owner keeps assisted conversations idle at most {ceiling} days"
                    ));
                }
                let before = rule.assisted(of);
                let kept = rule.accounts.iter().find(|set| set.of == *of).and_then(|set| set.grace_until);
                rule.accounts.retain(|set| set.of != *of);
                if let Some(days) = days {
                    rule.accounts.push(AccountDays { of: of.clone(), days, grace_until: kept });
                    // Its own grace alone: how it sets its own days holds
                    // back nobody else's conversations.
                    if sooner(before, rule.assisted(of))
                        && let Some(set) = rule.accounts.iter_mut().find(|set| set.of == *of)
                    {
                        set.grace_until = graced(kept);
                    }
                }
            }
            (RetentionAsk::Owner { .. }, Asker::Assisted(_)) => {
                return Err("only this machine's owner sets how long its conversations are kept idle".into());
            }
            (RetentionAsk::Mine { .. }, Asker::Owner) => {
                return Err("an assisted account sets its own fewer days; the owner sets them for all".into());
            }
        }
        rules.set(hub, rule);
        write(node, &rules).map_err(|error| error.to_string())?;
        rules
    };
    Ok(answer(node, hub, asker, &rules))
}

/// The rule of the network of `hub` as `asker` may know it, with when each
/// conversation it may see goes: the owner sees all those under the rule;
/// an assisted account its own and the ceiling.
fn answer(node: &Node, hub: &HubId, asker: &Asker, rules: &Rules) -> RetentionAnswer {
    let conversations = expiry::due(node, rules)
        .into_iter()
        .filter(|(_, acting)| match asker {
            Asker::Owner => expiry::under(node, hub, acting),
            Asker::Assisted(of) => matches!(acting, Acting::Accounts { accounts } if accounts.contains(of)),
        })
        .map(|(due, _)| due)
        .collect();
    let rule = rules.of(hub);
    let rule = match asker {
        Asker::Owner => rule,
        Asker::Assisted(of) => Rule {
            own_days: None,
            assisted_days: rule.assisted_days,
            accounts: rule.accounts.into_iter().filter(|set| set.of == *of).collect(),
            grace_until: rule.grace_until,
        },
    };
    RetentionAnswer { rule, owner: matches!(asker, Asker::Owner), conversations }
}

/// Asked over the network of `hub` by `account`: the machine's owner there,
/// or an account its agents assist; anyone else is refused.
pub(crate) fn remote(
    node: &Arc<Node>,
    view: &fastctx_net::directory::Directory,
    hub: &HubId,
    account: &AccountName,
    ask: RetentionAsk,
) -> Result<RetentionAnswer, Failure> {
    let me = node.machine();
    let asker = if view.member(&me).is_some_and(|member| member.home.as_ref() == Some(account)) {
        Asker::Owner
    } else if view.assisting(account, &me).next().is_some() {
        Asker::Assisted(HubAccount { hub: hub.clone(), account: account.clone() })
    } else {
        return Err(Failure::refused(format!(
            "only {me}'s owner and the accounts its agents assist ask of its retention"
        )));
    };
    asked(node, hub, &asker, ask).map_err(Failure::not_run)
}

/// The same of `machine`, this one or another of the network's, asked as
/// `account` where it is given.
pub(crate) async fn machine(
    node: &Arc<Node>,
    machine: Written,
    account: Option<HubAccount>,
    ask: RetentionAsk,
) -> Result<RetentionAnswer, Failure> {
    if node.is_here(&machine) {
        // This machine's user sets every network's rule, as its owner there;
        // as an account its agents assist, only its own.
        let hub = network_of(node, &machine, account.as_ref())?;
        let asker = match account {
            Some(account) if !expiry::owns(node, &account) => Asker::Assisted(account),
            _ => Asker::Owner,
        };
        let asking = node.clone();
        return fastctx_platform::task::spawn_blocking(move || asked(&asking, &hub, &asker, ask))
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(Failure::not_run);
    }
    let acting = match account {
        Some(account) => Acting::Accounts { accounts: vec![account] },
        None => Acting::Here,
    };
    let call = fastctx_types::remote::RemoteCall::ConversationRetention { ask };
    match node
        .call(&acting, &machine, fastctx_types::access::Purpose::Agents, call)
        .await
        .map_err(|error| Failure::new(error.to_string()))??
    {
        fastctx_types::remote::RemoteReply::ConversationRetention(answer) => Ok(answer),
        _ => Err(Failure::new(format!("{machine} answered something other than its retention"))),
    }
}

/// The network whose rule this machine's user asks of: the account's, the
/// one `machine` names, or the only one this machine is on.
fn network_of(node: &Node, machine: &Written, account: Option<&HubAccount>) -> Result<HubId, Failure> {
    if let Some(account) = account {
        return node
            .link_of(&account.hub)
            .map(|link| link.membership().id.clone())
            .ok_or_else(|| Failure::not_run(format!("this machine is not on the network of {}", account.account)));
    }
    let links: Vec<_> = node
        .links()
        .into_iter()
        .filter(|link| machine.hub.as_ref().is_none_or(|hub| link.hub_name() == *hub))
        .collect();
    match links.as_slice() {
        [link] => Ok(link.membership().id.clone()),
        [] => Err(Failure::not_run("this machine is on no such network")),
        _ => Err(Failure::not_run(format!(
            "this machine is on several networks: name the one, as {}.<network>",
            machine.name
        ))),
    }
}
