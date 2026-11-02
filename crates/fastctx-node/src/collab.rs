//! The union of the collaboration configured by the accounts a session
//! serves; on a computer in no network, its own (`crate::own_collab`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_net::link::Link;
use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::collab::{Caller, Collab, Offer, Offered, Started};
use fastctx_types::collab_prompt::DEFAULT_PROMPT;
use fastctx_types::ipc::Failure;
use fastctx_types::residents::Origin;
use fastctx_types::{AccountName, AgentName, ResidentId};

use crate::Node;
pub(crate) mod asked;
pub(crate) mod birth;

/// Accounts still opt in and select each calling login.
const OFFERED: bool = true;

pub(crate) struct Candidate {
    pub shown: String,
    pub whose: Whose,
    pub started: Started,
}

/// Whose settings offer a callee.
pub(crate) enum Whose {
    /// An account the session serves, at the hub of `link`.
    Account { link: Arc<Link>, account: AccountName },
    /// This computer's own, while it is in no network: the colleague works
    /// for this computer's user, as a session the user opened here does.
    Computer,
}

pub(crate) struct Choices {
    pub offered: Offered,
    pub candidates: Vec<Candidate>,
    pub acting: Acting,
}

struct Applies {
    link: Arc<Link>,
    account: AccountName,
    collab: Collab,
    label: String,
}

fn home_default(node: &Node, agent: &AgentName) -> Result<PathBuf, Failure> {
    if agent.as_str() == "codex" {
        return Ok(node.codex.default().config_home().to_path_buf());
    }
    let env = crate::claude::environment()?;
    let home = fastctx_platform::user_home().map_err(|error| Failure::new(error.to_string()))?;
    Ok(crate::homes::claude_default(&env, &home))
}

pub(crate) fn matches_home(
    node: &Node,
    agent: &AgentName,
    configured: Option<&Path>,
    actual: Option<&Path>,
) -> Result<bool, Failure> {
    let default = home_default(node, agent)?;
    Ok(crate::homes::same_dir(configured.unwrap_or(&default), actual.unwrap_or(&default)))
}

/// Listing before a session has called a tool uses only its login, without
/// creating a resident. A World-started resident's recorded identity wins.
pub(crate) async fn choices(
    node: &Arc<Node>,
    resident: Option<&ResidentId>,
    login: Option<&Caller>,
) -> Result<Option<Choices>, Failure> {
    let (node, resident, login) = (node.clone(), resident.cloned(), login.cloned());
    fastctx_platform::task::spawn_blocking(move || choices_now(&node, resident.as_ref(), login.as_ref()))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
}

fn choices_now(
    node: &Arc<Node>,
    resident: Option<&ResidentId>,
    login: Option<&Caller>,
) -> Result<Option<Choices>, Failure> {
    if !OFFERED {
        return Ok(None);
    }
    let about = resident.and_then(|id| node.residents.about(id).map(|(about, _)| about));
    // Only a conversation the World holds asks colleagues: a session the
    // user opened outside Fastctx is given `run` and nothing more
    // (`world-layers.md`).
    if !about.as_ref().is_some_and(|about| about.world_managed()) {
        return Ok(None);
    }
    let was_started = resident.map(|id| started(node, id)).transpose()?.flatten().is_some();
    if about.as_ref().is_some_and(|about| matches!(about.origin, Origin::Colleague { .. })) || was_started {
        return Ok(None);
    }
    let recorded = about.as_ref().map(|about| Caller { agent: about.host.agent.clone(), home: about.home.clone() });
    let login = match &about {
        Some(about) if about.world_started => recorded.as_ref(),
        _ => login,
    };
    let Some(login) = login else { return Ok(None) };
    if about.as_ref().is_some_and(|about| about.host.agent != login.agent) {
        return Err(Failure::not_run("the calling login does not match this session's agent"));
    }
    if !node.connected_anywhere() {
        return own(node, login);
    }
    let acting = resident.and_then(|id| crate::here::acting(node, id)).unwrap_or_default();
    let mut applies = Vec::new();
    for link in node.links() {
        let view = link.view();
        for account in node.accounts_on(&link, &acting) {
            let Some(found) = view.account(&account).filter(|found| !found.locked) else { continue };
            let Some(collab) = found.collab.as_ref().filter(|collab| collab.on) else { continue };
            let mut calling = false;
            for caller in &collab.callers {
                if view.is_me(&caller.machine)
                    && caller.agent == login.agent
                    && matches_home(node, &login.agent, caller.home.as_deref(), login.home.as_deref())?
                    && view.may_use_login(&account, &caller.machine, &caller.agent, caller.home.as_deref())
                {
                    calling = true;
                    break;
                }
            }
            if calling {
                applies.push(Applies {
                    link: link.clone(),
                    label: account.to_string(),
                    account,
                    collab: collab.clone(),
                });
            }
        }
    }
    if applies.is_empty() {
        return Ok(None);
    }
    applies.sort_by_key(|item| (item.link.hub_name(), item.account.clone()));
    for i in 0..applies.len() {
        if applies.iter().filter(|item| item.account == applies[i].account).count() > 1 {
            applies[i].label = format!("{}.{}", applies[i].account, applies[i].link.hub_name());
        }
    }
    let mut texts: Vec<(String, Vec<String>)> = Vec::new();
    for item in &applies {
        let text = item.collab.prompt.as_deref().unwrap_or(DEFAULT_PROMPT).to_string();
        match texts.iter_mut().find(|(existing, _)| *existing == text) {
            Some((_, accounts)) => accounts.push(item.label.clone()),
            None => texts.push((text, vec![item.label.clone()])),
        }
    }
    let prompt = if texts.len() == 1 {
        texts.remove(0).0
    } else {
        texts
            .into_iter()
            .map(|(text, accounts)| format!("{}:\n{text}", accounts.join(", ")))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let mut candidates = Vec::new();
    let mut callees = Vec::new();
    let mut counts = std::collections::HashMap::new();
    for callee in applies.iter().flat_map(|item| &item.collab.callees) {
        *counts.entry(&callee.name).or_insert(0_usize) += 1;
    }
    for item in &applies {
        let view = item.link.view();
        for callee in &item.collab.callees {
            let repeated = counts[&callee.name] > 1;
            let shown = if repeated { format!("{}@{}", callee.name, item.label) } else { callee.name.clone() };
            let hands = view.rights(&item.account, &callee.login.machine).hands;
            callees.push(Offer {
                name: shown.clone(),
                account: Some(item.label.clone()),
                use_when: callee.use_when.clone(),
                lives_on: hands.then(|| node.written(&item.link, &callee.login.machine).to_string()),
                at_once: fastctx_types::collab::AtOnce::most(callee.at_once),
            });
            candidates.push(Candidate {
                shown,
                whose: Whose::Account { link: item.link.clone(), account: item.account.clone() },
                started: Started { callee: callee.clone(), rules: item.collab.rules.clone() },
            });
        }
    }
    let accounts = applies
        .iter()
        .map(|item| HubAccount { hub: item.link.membership().id.clone(), account: item.account.clone() })
        .collect();
    Ok(Some(Choices { offered: Offered { callees, prompt }, candidates, acting: Acting::Accounts { accounts } }))
}

/// What this computer's own settings offer `login`, while it is in no
/// network: its callers and callees are its own logins, whatever this
/// computer was called when they were written.
fn own(node: &Node, login: &Caller) -> Result<Option<Choices>, Failure> {
    let collab = crate::own_collab::read(&node.world)?;
    if !collab.on {
        return Ok(None);
    }
    let mut calling = false;
    for caller in &collab.callers {
        calling |= caller.agent == login.agent
            && matches_home(node, &login.agent, caller.home.as_deref(), login.home.as_deref())?;
    }
    if !calling {
        return Ok(None);
    }
    let here = node.machine();
    let prompt = collab.prompt.clone().unwrap_or_else(|| DEFAULT_PROMPT.to_string());
    let mut callees = Vec::new();
    let mut candidates = Vec::new();
    for callee in &collab.callees {
        let mut callee = callee.clone();
        callee.login.machine = here.clone();
        callees.push(Offer {
            name: callee.name.clone(),
            account: None,
            use_when: callee.use_when.clone(),
            lives_on: Some(here.to_string()),
            at_once: fastctx_types::collab::AtOnce::most(callee.at_once),
        });
        candidates.push(Candidate {
            shown: callee.name.clone(),
            whose: Whose::Computer,
            started: Started { callee, rules: collab.rules.clone() },
        });
    }
    Ok(Some(Choices { offered: Offered { callees, prompt }, candidates, acting: Acting::Here }))
}

pub(crate) async fn require(
    node: &Arc<Node>,
    resident: &ResidentId,
    login: Option<&Caller>,
) -> Result<Choices, Failure> {
    let (node, resident, login) = (node.clone(), resident.clone(), login.cloned());
    fastctx_platform::task::spawn_blocking(move || require_now(&node, &resident, login.as_ref()))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
}

fn require_now(node: &Arc<Node>, resident: &ResidentId, login: Option<&Caller>) -> Result<Choices, Failure> {
    if let Some((about, _)) = node.residents.about(resident)
        && matches!(about.origin, Origin::Colleague { .. })
    {
        return Err(Failure::not_run(fastctx_types::collab_prompt::NO_NESTING));
    }
    choices_now(node, Some(resident), login)?.ok_or_else(|| {
        Failure::not_run(if !node.residents.about(resident).is_some_and(|(about, _)| about.world_managed()) {
            crate::speech::OWN_SESSION
        } else if OFFERED {
            "model collaboration is off for this caller; the user changes it on the Model collaboration page"
        } else {
            "model collaboration is still in development: no colleague can be asked yet"
        })
    })
}

/// Checks the durable relationship budget before starting another colleague.
pub(crate) async fn room_for_ask(node: &Arc<Node>, asker: &ResidentId) -> Result<(), Failure> {
    let directory = node.world.conversation(asker);
    fastctx_platform::task::spawn_blocking(move || asked::available(&directory))
        .await
        .map_err(|error| Failure::not_run(error.to_string()))?
        .map_err(|error| Failure::not_run(error.to_string()))
}

/// Pins the chosen account for future messages to this colleague.
pub(crate) async fn keep_asked(
    node: &Arc<Node>,
    asker: &ResidentId,
    colleague: &fastctx_types::Address,
    account: &HubAccount,
) -> Result<(), Failure> {
    let (directory, colleague, account) = (node.world.conversation(asker), colleague.clone(), account.clone());
    fastctx_platform::task::spawn_blocking(move || {
        asked::keep(&directory, &colleague, &account).map_err(|error| {
            Failure::new(format!("colleague {colleague} started, but recording its account failed: {error}"))
        })
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?
}

pub(crate) async fn asked_account(
    node: &Arc<Node>,
    asker: &ResidentId,
    colleague: &fastctx_types::Address,
) -> Result<Option<HubAccount>, Failure> {
    let (directory, colleague) = (node.world.conversation(asker), colleague.clone());
    fastctx_platform::task::spawn_blocking(move || asked::get(&directory, &colleague))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(format!("reading the colleague's account: {error}")))
}

pub(crate) async fn keep(node: &Arc<Node>, resident: &ResidentId, started: &Started) -> Result<(), Failure> {
    let (directory, started) = (node.world.conversation(resident), started.clone());
    fastctx_platform::task::spawn_blocking(move || birth::keep(&directory, &started))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(format!("keeping the callee's configuration: {error}")))
}

pub(crate) fn started(node: &Node, resident: &ResidentId) -> Result<Option<Started>, Failure> {
    birth::read(&node.world.conversation(resident))
        .map_err(|error| Failure::new(format!("reading the callee's configuration: {error}")))
}
