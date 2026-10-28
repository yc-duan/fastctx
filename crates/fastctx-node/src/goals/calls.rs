//! User controls and resident commands retain their distinct authority at the account boundary.

use super::{operations, scope, words};
use crate::{
    Node,
    town::caller::{self, Caller},
};
use fastctx_types::access::{Acting, HubAccount, Purpose, Written};
use fastctx_types::goals::{AgentRequest, Author, Request, Updated};
use fastctx_types::ipc::{Failure, Peer};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::{AccountName, Address, HubName, JobId, ResidentId};
use std::sync::Arc;

pub(crate) async fn user(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<Written>,
    account: Option<HubAccount>,
    request: Request,
) -> Result<Updated, Failure> {
    let machine = match machine {
        Some(machine) => machine,
        None if node.residents.knows(&resident) || node.residents.deleted(&resident).is_some() => node.machine().into(),
        None => crate::colleagues::find_for(node, &Acting::Here, &resident).await?,
    };
    if node.is_here(&machine) {
        let account = scope::resolve(node, &resident, account.as_ref())?;
        return user_here(node, resident, account, node.machine(), request).await;
    }
    let acting = account
        .as_ref()
        .map(|account| Acting::one(account.hub.clone(), account.account.clone()))
        .unwrap_or(Acting::Here);
    if let Some(account) = &account
        && !node.serves(&Acting::Here, &account.hub, &account.account)
    {
        return Err(Failure::refused(words::ACCOUNT_MISSING));
    }
    let purpose = if matches!(request, Request::Read | Request::History { .. } | Request::Past) {
        Purpose::Look
    } else {
        Purpose::Agents
    };
    let mut last = Failure::not_run(words::OLDER_HUB);
    for (link, account) in node.speakers(&acting, &machine, purpose).map_err(failure)? {
        if !link.view().goal_control {
            continue;
        }
        let facts = link.call(&machine.name, &account, Purpose::Look, RemoteCall::Facts).await.map_err(failure)??;
        if !matches!(facts, RemoteReply::Facts(facts) if facts.goal_runtime) {
            last = Failure::not_run(words::OLDER_NODE);
            continue;
        }
        match link
            .call(
                &machine.name,
                &account,
                purpose,
                RemoteCall::Goal { resident: resident.clone(), request: request.clone() },
            )
            .await
            .map_err(failure)?
        {
            Ok(RemoteReply::Goal { updated }) => return Ok(*updated),
            Ok(_) => return Err(Failure::new(fastctx_types::goals::words::EXPECTED_GOAL)),
            Err(failure) if failure.not_run => last = failure,
            Err(failure) => return Err(failure),
        }
    }
    Err(last)
}

pub(crate) async fn user_here(
    node: &Arc<Node>,
    resident: ResidentId,
    account: HubAccount,
    machine: fastctx_types::MachineName,
    request: Request,
) -> Result<Updated, Failure> {
    if matches!(request, Request::Mark { .. }) {
        return Err(Failure::refused(words::MAIN_ONLY));
    }
    let account = scope::resolve(node, &resident, Some(&account))?;
    require(node, &request)?;
    let address = Address { machine: node.machine(), resident };
    operations::call(node, address, account, Author::User { machine }, None, request).await
}

pub(crate) async fn command(
    node: &Arc<Node>,
    peer: &Peer,
    hub: Option<HubName>,
    account: Option<AccountName>,
    job: Option<JobId>,
    request: AgentRequest,
) -> Result<Updated, Failure> {
    let Caller::Resident(address) = caller::of(node, peer)? else {
        return Err(Failure::refused(words::MAIN_ONLY));
    };

    let (selected, turn) = if let Some(job) = job {
        let directory = node.world.owner(&node.names(), &address);
        let record = fastctx_hands::run::record::read(&directory.job_record(&job)).map_err(failure)?;
        if record.end.is_some() || record.owner.resident().is_none_or(|owner| !super::same(node, owner, &address)) {
            return Err(Failure::refused(fastctx_types::goals::words::INACTIVE_JOB));
        }
        (record.on.as_ref().and_then(|on| on.account.clone()).or(record.account), record.turn)
    } else if node.is_me(&address.machine) {
        (None, node.residents.now(&address.resident).and_then(|now| now.turn))
    } else {
        return Err(Failure::refused(fastctx_types::goals::words::REMOTE_JOB));
    };
    let selected = choose(node, &address, hub.as_ref(), account.as_ref(), selected.as_ref())?;
    if !node.is_me(&address.machine)
        && selected
            .as_ref()
            .is_none_or(|selected| !crate::jobs::running_for(&node.world, &node.names(), &address).contains(selected))
    {
        return Err(Failure::refused(words::ACCOUNT_MISSING));
    }
    let (target, scope) = if let AgentRequest::ControlColleague { colleague, .. } = &request {
        super::commands::target(node, &address, colleague, selected).await?
    } else {
        let scope = if node.is_me(&address.machine) {
            scope::resolve(node, &address.resident, selected.as_ref())?
        } else {
            selected.ok_or_else(|| Failure::refused(words::ACCOUNT_MISSING))?
        };
        (address.clone(), scope)
    };
    super::commands::route(node, target, address, scope, turn, request).await
}

fn choose(
    node: &Node,
    address: &Address,
    hub: Option<&HubName>,
    account: Option<&AccountName>,
    job_account: Option<&HubAccount>,
) -> Result<Option<HubAccount>, Failure> {
    if hub.is_none() && account.is_none() {
        return Ok(job_account.cloned());
    }
    let acting = crate::around::acting_of(node, &Caller::Resident(address.clone()));
    let mut choices = Vec::new();
    for link in node.links().into_iter().filter(|link| hub.is_none_or(|hub| link.hub_name() == *hub)) {
        for name in
            node.accounts_on(&link, &acting).into_iter().filter(|name| account.is_none_or(|account| account == name))
        {
            let scope = HubAccount { hub: link.membership().id.clone(), account: name };
            if job_account.is_none_or(|job| job == &scope) {
                choices.push(scope);
            }
        }
    }
    match choices.len() {
        1 => Ok(choices.pop()),
        0 => Err(Failure::refused(words::ACCOUNT_MISSING)),
        _ => Err(Failure::not_run(words::ACCOUNT_AMBIGUOUS)),
    }
}

fn require(node: &Node, request: &Request) -> Result<(), Failure> {
    if !node.goals.records.available() {
        return Err(Failure::not_run(words::RECORDS_MISSING));
    }
    if !matches!(request, Request::Read | Request::History { .. } | Request::Past) && !node.goals.context.available() {
        return Err(Failure::not_run(words::CONTEXT_MISSING));
    }
    Ok(())
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
