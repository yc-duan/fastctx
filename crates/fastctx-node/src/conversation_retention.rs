//! The rule this machine deletes its conversations idle long enough by
//! (`fastctx_types::conversation_retention`): kept in
//! `<world>/conversation-retention.json`, read and set by the machine's owner
//! and by the accounts its agents assist, each for its own, and applied here
//! by `expiry`.

pub(crate) mod expiry;

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use fastctx_types::AccountName;
use fastctx_types::access::{Acting, HubAccount, HubId};
use fastctx_types::conversation_retention::{AccountDays, GRACE_DAYS, RetentionAnswer, RetentionAsk, Rule, problem};
use fastctx_types::ipc::Failure;

use crate::Node;

/// Held while the rule is read and written again.
static WRITING: Mutex<()> = Mutex::new(());

fn path(node: &Node) -> PathBuf {
    node.world.path().join("conversation-retention.json")
}

/// The rule as kept; off where none is.
pub(crate) fn read(node: &Node) -> io::Result<Rule> {
    match fastctx_platform::disk::read(path(node)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Rule::default()),
        Err(error) => Err(error),
    }
}

fn write(node: &Node, rule: &Rule) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(rule).map_err(io::Error::other)?;
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

/// Holds back every deletion `GRACE_DAYS` from now, for a rule that came to
/// delete sooner: whoever it touches sees it coming.
fn graced(rule: &mut Rule) {
    let until = jiff::Timestamp::now() + jiff::SignedDuration::from_hours(GRACE_DAYS * 24);
    rule.grace_until = Some(rule.grace_until.map_or(until, |grace| grace.max(until)));
}

/// Who asks of the rule.
pub(crate) enum Asker {
    /// The machine's owner, or its user here.
    Owner,
    /// An account the machine's agents assist.
    Assisted(HubAccount),
}

/// Answers `ask` for `asker`. Call on a thread that may block.
pub(crate) fn asked(node: &Arc<Node>, asker: &Asker, ask: RetentionAsk) -> Result<RetentionAnswer, String> {
    let rule = {
        let _writing = WRITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut rule = read(node).map_err(|error| error.to_string())?;
        match (ask, asker) {
            (RetentionAsk::Read, _) => {}
            (RetentionAsk::Owner { own_days, assisted_days }, Asker::Owner) => {
                if let Some(problem) = problem(own_days).or_else(|| problem(assisted_days)) {
                    return Err(problem);
                }
                if sooner(rule.own_days, own_days) || sooner(rule.assisted_days, assisted_days) {
                    graced(&mut rule);
                }
                rule.own_days = own_days;
                rule.assisted_days = assisted_days;
                write(node, &rule).map_err(|error| error.to_string())?;
            }
            (RetentionAsk::Mine { days }, Asker::Assisted(of)) => {
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
                rule.accounts.retain(|set| set.of != *of);
                if let Some(days) = days {
                    rule.accounts.push(AccountDays { of: of.clone(), days });
                }
                if sooner(before, rule.assisted(of)) {
                    graced(&mut rule);
                }
                write(node, &rule).map_err(|error| error.to_string())?;
            }
            (RetentionAsk::Owner { .. }, Asker::Assisted(_)) => {
                return Err("only this machine's owner sets how long its conversations are kept idle".into());
            }
            (RetentionAsk::Mine { .. }, Asker::Owner) => {
                return Err("an assisted account sets its own fewer days; the owner sets them for all".into());
            }
        }
        rule
    };
    Ok(answer(node, asker, rule))
}

/// The rule as `asker` may know it, with when each conversation it may see
/// goes: the owner sees all; an assisted account its own and the ceiling.
fn answer(node: &Node, asker: &Asker, rule: Rule) -> RetentionAnswer {
    let conversations = expiry::due(node, &rule)
        .into_iter()
        .filter(|(_, acting)| match asker {
            Asker::Owner => true,
            Asker::Assisted(of) => matches!(acting, Acting::Accounts { accounts } if accounts.contains(of)),
        })
        .map(|(due, _)| due)
        .collect();
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
    asked(node, &asker, ask).map_err(Failure::not_run)
}

/// The same of `machine`, this one or another of the network's.
pub(crate) async fn machine(
    node: &Arc<Node>,
    machine: fastctx_types::access::Written,
    ask: RetentionAsk,
) -> Result<RetentionAnswer, Failure> {
    if node.is_here(&machine) {
        let asking = node.clone();
        return fastctx_platform::task::spawn_blocking(move || asked(&asking, &Asker::Owner, ask))
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(Failure::not_run);
    }
    let call = fastctx_types::remote::RemoteCall::ConversationRetention { ask };
    match node
        .call(&Acting::Here, &machine, fastctx_types::access::Purpose::Agents, call)
        .await
        .map_err(|error| Failure::new(error.to_string()))??
    {
        fastctx_types::remote::RemoteReply::ConversationRetention(answer) => Ok(answer),
        _ => Err(Failure::new(format!("{machine} answered something other than its retention"))),
    }
}
