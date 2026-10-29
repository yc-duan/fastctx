//! Register, list and withdraw the calling account's sentinels.
use super::transport::Transport;
use crate::{Node, town::caller::Caller};
use fastctx_net::link::Link;
use fastctx_types::{
    AccountName, HubName, access::HubAccount, ipc::Failure, sentinel::Definition, sentinel_control::*,
    sentinel_settings::HomeStatus, sentinel_words as words,
};
use std::sync::Arc;

/// Native conversations use the accounts currently logged in here, just as run
/// does. Explicitly delegated conversations retain their recorded account scope.
pub(crate) fn scope(
    node: &Node,
    caller: &Caller,
    hub: Option<&HubName>,
    account: Option<&AccountName>,
) -> Result<(Arc<Link>, AccountName), Failure> {
    if matches!(caller, Caller::Page(_) | Caller::Session(_)) {
        return Err(Failure::refused(words::NEED_CONVERSATION));
    }
    let mut choices = scopes(node, caller, hub, account);
    if choices.len() != 1 {
        return Err(Failure::not_run(words::CHOOSE_ACCOUNT));
    }
    Ok(choices.pop().expect("one account"))
}
pub(super) fn scopes(
    node: &Node,
    caller: &Caller,
    hub: Option<&HubName>,
    account: Option<&AccountName>,
) -> Vec<(Arc<Link>, AccountName)> {
    let acting = crate::around::acting_of(node, caller);
    let mut choices = Vec::new();
    for link in node.links().into_iter().filter(|link| hub.is_none_or(|name| link.hub_name() == *name)) {
        for name in node.accounts_on(&link, &acting) {
            if account.is_none_or(|wanted| *wanted == name) {
                choices.push((link.clone(), name));
            }
        }
    }
    choices
}

pub(crate) async fn list_all(
    node: &Arc<Node>,
    caller: &Caller,
    owner: Option<fastctx_types::Address>,
) -> Result<Reply, Failure> {
    if matches!(caller, Caller::Page(_) | Caller::Session(_)) {
        return Err(Failure::refused(words::NEED_CONVERSATION));
    }
    tokio::time::timeout(std::time::Duration::from_secs(75), async {
        let mut sentinels = Vec::new();
        let mut unavailable = Vec::new();
        for (link, account) in scopes(node, caller, None, None) {
            match control(node, caller, &link, &account, Request::List { owner: owner.clone() }).await {
                Ok(Reply::Listed { sentinels: found, unavailable: errors }) => {
                    sentinels.extend(found);
                    unavailable.extend(errors);
                }
                Ok(_) => return Err(Failure::new("unexpected sentinel list reply")),
                Err(error) => unavailable.push(format!("{} at {}: {}", account, link.hub_name(), error.message)),
            }
        }
        Ok(Reply::Listed { sentinels, unavailable })
    })
    .await
    .map_err(|_| Failure::new("listing sentinels across accounts timed out"))?
}

/// Every sentinel of the accounts the caller acts for, ended or not, with
/// their wakes (`world-senses.md`, 全部观测).
pub(crate) async fn overview_all(node: &Arc<Node>, caller: &Caller) -> Result<Reply, Failure> {
    if matches!(caller, Caller::Page(_) | Caller::Session(_)) {
        return Err(Failure::refused(words::NEED_CONVERSATION));
    }
    tokio::time::timeout(std::time::Duration::from_secs(120), async {
        let mut watched = Vec::new();
        let mut unavailable = Vec::new();
        for (link, account) in scopes(node, caller, None, None) {
            match control(node, caller, &link, &account, Request::Overview).await {
                Ok(Reply::Overview { watched: found, unavailable: errors }) => {
                    watched.extend(found);
                    unavailable.extend(errors);
                }
                Ok(_) => return Err(Failure::new("unexpected sentinel overview reply")),
                Err(error) => unavailable.push(format!("{} at {}: {}", account, link.hub_name(), error.message)),
            }
        }
        Ok(Reply::Overview { watched, unavailable })
    })
    .await
    .map_err(|_| Failure::new("reading sentinels across accounts timed out"))?
}

/// A sentinel's record, read for whichever of the caller's accounts it was
/// left for: each keeps its own under its home.
pub(crate) async fn record_any(node: &Arc<Node>, caller: &Caller, request: Request) -> Result<Reply, Failure> {
    if matches!(caller, Caller::Page(_) | Caller::Session(_)) {
        return Err(Failure::refused(words::NEED_CONVERSATION));
    }
    let mut failed = None;
    for (link, account) in scopes(node, caller, None, None) {
        match control(node, caller, &link, &account, request.clone()).await {
            Ok(reply) => return Ok(reply),
            Err(failure) => failed = Some(failure),
        }
    }
    Err(failed.unwrap_or_else(|| Failure::not_run(words::CHOOSE_ACCOUNT)))
}

pub(crate) async fn control(
    node: &Arc<Node>,
    caller: &Caller,
    link: &Arc<Link>,
    account: &AccountName,
    request: Request,
) -> Result<Reply, Failure> {
    let scope = HubAccount { hub: link.membership().id.clone(), account: account.clone() };
    let records = node.sentinels.records()?;
    let homes = Transport(node.clone());
    let fail = |error: fastctx_sentinel::records::Error| Failure::new(error.to_string());
    match request {
        Request::Help => {
            let (_, settings) = fastctx_sentinel::settings::read(records, &scope).await.map_err(fail)?;
            // Without a home the help still reads true, and says what is missing first.
            let text = match settings.home {
                Some(home) => words::HELP.replace("<machine>", home.as_str()),
                None => format!("{}.\n\n{}", words::NO_HOME, words::HELP.replace("<machine>", "the machine the user chooses for them")),
            };
            Ok(Reply::Help { text })
        }
        Request::Record { id, home, generation, held } => {
            match homes.send_to(&scope, &home, HomeRequest::Record { id, generation, held }).await.map_err(Failure::new)? {
                HomeReply::Record { record } => Ok(Reply::Record { record }),
                _ => Err(Failure::new("the sentinel home answered another kind of call")),
            }
        }
        // The account's sentinels live on the home it chose; a home that
        // cannot be asked is said, and the rest is told.
        Request::Overview => {
            let (_, settings) = fastctx_sentinel::settings::read(records, &scope).await.map_err(fail)?;
            let Some(home) = settings.home else {
                return Ok(Reply::Overview { watched: Vec::new(), unavailable: Vec::new() });
            };
            match homes.send_to(&scope, &home, HomeRequest::Overview).await {
                Ok(HomeReply::Overview { watched }) => Ok(Reply::Overview { watched, unavailable: Vec::new() }),
                Ok(_) => Err(Failure::new("the sentinel home answered another kind of call")),
                Err(error) => Ok(Reply::Overview { watched: Vec::new(), unavailable: vec![format!("{home}: {error}")] }),
            }
        }
        Request::Create { mut request } => {
            let Caller::Resident(owner) = caller else {
                return Err(Failure::refused(words::NEED_CONVERSATION));
            };
            // A sentinel wakes the conversation it reports to: only one the
            // World holds leaves one (`world-layers.md`), which only the
            // machine it lives on can tell.
            if !node.is_me(&owner.machine) {
                return Err(Failure::refused(words::ON_ITS_MACHINE));
            }
            if node.residents.about(&owner.resident).is_none_or(|(about, _)| !about.world_managed()) {
                return Err(Failure::refused(crate::speech::OWN_SESSION));
            }
            if let Some((about, _)) = node.residents.about(&owner.resident)
                && about.host.agent.as_str() == "codex"
                && !about.world_started
            {
                crate::codex::native_lifetime::processes(&node.world, &owner.resident).map_err(Failure::not_run)?;
            }
            let now = jiff::Timestamp::now().as_millisecond();
            request.validate(now).map_err(Failure::not_run)?;
            let (_, settings) = fastctx_sentinel::settings::read(records, &scope).await.map_err(fail)?;
            let home = settings.home.ok_or_else(|| Failure::not_run(words::NO_HOME))?;
            let status = super::settings::home_status(link, account, Some(&home)).await;
            if status != HomeStatus::Ready {
                return Err(Failure::not_run(words::home_unavailable(&status)));
            }
            if !request.condition.trim().is_empty() {
                if settings.model.is_none() {
                    return Err(Failure::not_run(words::NO_MODEL));
                }
                let credentials = node
                    .sentinels
                    .credentials
                    .get()
                    .ok_or_else(|| Failure::not_run("sentinel credential service is not connected"))?;
                let keys = tokio::time::timeout(std::time::Duration::from_secs(30), credentials.status(&scope, &home))
                    .await
                    .map_err(|_| Failure::not_run("sentinel key status timed out"))?
                    .map_err(Failure::not_run)?;
                if !keys.model {
                    return Err(Failure::not_run(words::NO_KEY));
                }
            }
            request.at.sort_unstable();
            request.at.dedup();
            let mut bytes = [0u8; 8];
            getrandom::fill(&mut bytes).map_err(|e| Failure::new(e.to_string()))?;
            let id = format!("s-{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
                .parse()
                .map_err(|e: fastctx_types::NameError| Failure::new(e.to_string()))?;
            let kind = request.kind();
            let definition = Definition {
                id,
                hub: link.hub_name(),
                account: account.clone(),
                owner: owner.clone(),
                home,
                generation: 1,
                condition: request.condition,
                at: request.at,
                kind,
                tell: request.tell,
            };
            let sentinel =
                fastctx_sentinel::lifecycle::create(records, &homes, Registration { scope, definition, active: true })
                    .await
                    .map_err(fail)?;
            Ok(Reply::Created { sentinel: Box::new(sentinel) })
        }
        Request::List { owner } => {
            let view = crate::authorization::current_view(link)?;
            let include = |candidate: &fastctx_types::Address| {
                owner.as_ref().is_none_or(|owner| {
                    owner.resident == candidate.resident && view.same(&owner.machine, &candidate.machine)
                })
            };
            Ok(Reply::Listed {
                sentinels: fastctx_sentinel::lifecycle::list_where(records, &homes, &scope, &include)
                    .await
                    .map_err(fail)?,
                unavailable: Vec::new(),
            })
        }
        Request::Stop { id, home } => {
            let Some((_, registration)) = fastctx_sentinel::lifecycle::get(records, &scope, &id).await.map_err(fail)?
            else {
                return Err(Failure::not_run("sentinel not found"));
            };
            let view = crate::authorization::current_view(link)?;
            if home.as_ref().is_some_and(|home| !view.same(home, &registration.definition.home)) {
                return Err(Failure::not_run("the sentinel lives on a different machine"));
            }
            let owner = match caller {
                Caller::Resident(owner) => {
                    if owner.resident != registration.definition.owner.resident
                        || !view.same(&owner.machine, &registration.definition.owner.machine)
                    {
                        return Err(Failure::refused("this conversation does not own the sentinel"));
                    }
                    Some(&registration.definition.owner)
                }
                Caller::User => None,
                _ => return Err(Failure::refused(words::NEED_CONVERSATION)),
            };
            let reason = fastctx_sentinel::lifecycle::stop(records, &homes, &scope, &id, owner).await.map_err(fail)?;
            Ok(Reply::Stopped { id, home_confirmed: reason.is_none(), reason })
        }
    }
}
