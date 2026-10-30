//! Plans are stored on the hub, written there from each conversation's plan
//! file (`plan_file`); only a conversation's observed revision lives here.
//! A command reads the plan; it changes as its file does.

use std::sync::Arc;

use crate::{Node, town::caller::Caller};
use fastctx_types::{
    AccountName, Address, HubName, ResidentId,
    access::{Acting, HubAccount, Purpose},
    ipc::Failure,
    plan::{self, Author, Operation, Page, words},
    remote::{RemoteCall, RemoteReply},
    residents::{About, Origin},
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Seen {
    account: HubAccount,
    owner: ResidentId,
    version: Option<u64>,
}

fn seen(node: &Node, id: &ResidentId) -> Result<Option<Seen>, Failure> {
    match std::fs::read(node.world.conversation(id).path().join("plan-seen.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| Failure::new(error.to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Failure::new(error.to_string())),
    }
}

fn remember(node: &Node, id: &ResidentId, account: HubAccount, page: &Page) -> Result<(), Failure> {
    let observed = Seen { account, owner: page.owner.clone(), version: page.version() };
    keep_seen(node, id, &observed)
}

fn keep_seen(node: &Node, id: &ResidentId, observed: &Seen) -> Result<(), Failure> {
    let _transaction = crate::resident_storage::lock();
    if seen(node, id)?.is_some_and(|before| {
        before.account == observed.account && before.owner == observed.owner && before.version >= observed.version
    }) {
        return Ok(());
    }
    let bytes = serde_json::to_vec(observed).map_err(|error| Failure::new(error.to_string()))?;
    fastctx_platform::publish(
        &node.world.conversation(id).path().join("plan-seen.json"),
        &bytes,
        fastctx_platform::Permissions::Private,
    )
    .map_err(|error| Failure::new(error.to_string()))
}

pub(crate) async fn call(
    node: &Arc<Node>,
    caller: &Caller,
    hub: Option<HubName>,
    account: Option<AccountName>,
    operation: Operation,
) -> Result<plan::Result, Failure> {
    let Caller::Resident(address) = caller else {
        return Err(Failure::not_run(words::RESIDENT));
    };
    if node.is_me(&address.machine) {
        return here(node, &address.resident, hub.as_ref(), account.as_ref(), None, operation).await;
    }
    let mut choices = Vec::new();
    for acting in crate::jobs::running_for(&node.world, &node.names(), address) {
        let Some(link) = node.link_of(&acting.hub) else { continue };
        if hub.as_ref().is_some_and(|hub| link.hub_name() != *hub)
            || account.as_ref().is_some_and(|account| *account != acting.account)
        {
            continue;
        }
        link.refresh().await.map_err(Failure::new)?;
        let view = link.view();
        if view.admits(&view.me, &acting.account, &address.machine, Purpose::Echo).is_ok() {
            choices.push((link, acting));
        }
    }
    if choices.len() != 1 {
        return Err(Failure::refused(words::ROUTE));
    }
    let (link, acting) = choices.pop().expect("one route");
    if !link.view().conversation_plans {
        return Err(Failure::not_run(words::OLDER));
    }
    let call = RemoteCall::Plan { resident: address.resident.clone(), operation };
    match link
        .call(&address.machine, &acting.account, Purpose::Echo, call)
        .await
        .map_err(|error| Failure::new(error.to_string()))??
    {
        RemoteReply::Plan { result } => Ok(result),
        _ => Err(Failure::new(words::REPLY)),
    }
}

pub(crate) async fn here(
    node: &Arc<Node>,
    id: &ResidentId,
    hub: Option<&HubName>,
    account: Option<&AccountName>,
    requesting: Option<&HubAccount>,
    operation: Operation,
) -> Result<plan::Result, Failure> {
    let gate = node.residents.management(id)?;
    let _one = crate::gate_waits::alone(node, id, &gate).await?;
    node.residents.active(id)?;
    crate::context::plugin_heard(node, id).map_err(|error| Failure::new(error.to_string()))?;
    let (about, _) = node.residents.about(id).ok_or_else(|| Failure::refused(words::RESIDENT))?;
    let (whose, colleague) = match &about.origin {
        Origin::Colleague { asked_by, .. } => (asked_by.clone(), true),
        Origin::User => (Address { machine: about.machine.clone(), resident: id.clone() }, false),
    };
    let owner = whose.resident.clone();
    if colleague && matches!(operation, Operation::Write { .. }) {
        return Err(Failure::refused(words::READ_ONLY));
    }
    let previous = seen(node, id)?;
    let caller = Caller::Resident(Address { machine: node.machine(), resident: id.clone() });
    let selected = requesting.or_else(|| {
        if hub.is_none() && account.is_none() { previous.as_ref().map(|seen| &seen.account) } else { None }
    });
    let (link, account) = match selected {
        Some(acting) => {
            if !node.serves(&about.acting, &acting.hub, &acting.account) {
                return Err(Failure::refused(words::ROUTE));
            }
            let link = node.link_of(&acting.hub).ok_or_else(|| Failure::refused(words::ROUTE))?;
            (link, acting.account.clone())
        }
        None => crate::profile::scope(node, &caller, hub, account)?,
    };
    let scope = HubAccount { hub: link.membership().id, account: account.clone() };
    let result = match operation {
        Operation::Read => {
            let page = link.plan_read(&account, owner).await.map_err(Failure::new)?;
            if let Some(other) = link.plan_of_another(&page, &whose) {
                return Err(Failure::not_run(words::elsewhere(other)));
            }
            if page.deleted() {
                return Err(Failure::new(words::DELETED));
            }
            plan::Result::Page { page }
        }
        // The plan changes as its file does; an older command that writes
        // it whole is told where the file is.
        Operation::Write { .. } => {
            let _ = (previous, scope);
            let location = crate::placement::location::prepare(node, id, &about.origin, &about.acting)
                .await?
                .ok_or_else(|| Failure::not_run("there is no online working computer for the plan file"))?;
            return Err(Failure::not_run(words::file(
                std::path::Path::new(&crate::placement::plan_path(&location)),
                &location.machine,
            )));
        }
    };
    if let plan::Result::Page { page } = &result {
        let version =
            serde_json::to_string(&Seen { account: scope.clone(), owner: page.owner.clone(), version: page.version() })
                .map_err(|error| Failure::new(error.to_string()))?;
        crate::context::plan(node, id, page, &version).map_err(|error| Failure::new(error.to_string()))?;
        remember(node, id, scope, page)?;
    }
    Ok(result)
}

/// Refreshes the plan projection without advancing the conversation's observed
/// revision. A native conversation with several accounts selects one by reading
/// its plan explicitly; context must not choose an account on its behalf.
pub(crate) async fn context(
    node: &Arc<Node>,
    id: &ResidentId,
    origin: &Origin,
    acting: &Acting,
) -> Result<(), Failure> {
    let previous = seen(node, id)?;
    let selected = previous.as_ref().map(|seen| seen.account.clone()).or_else(|| only_account(node, acting));
    let unavailable = || crate::context::plan_unavailable(node, id).map_err(|error| Failure::new(error.to_string()));
    let Some(scope) = selected else { return unavailable() };
    if !node.serves(acting, &scope.hub, &scope.account) {
        unavailable()?;
        return Err(Failure::refused(words::ROUTE));
    }
    let Some(link) = node.link_of(&scope.hub) else {
        unavailable()?;
        return Err(Failure::new(words::ROUTE));
    };
    let view = link.view();
    if !view.account_records || !view.conversation_plans {
        return unavailable();
    }
    let whose = match origin {
        Origin::User => Address { machine: node.machine(), resident: id.clone() },
        Origin::Colleague { asked_by, .. } => asked_by.clone(),
    };
    let Some(generation) = node.plan_reads.begin(id, &scope, &whose) else { return Ok(()) };
    // A hub that does not answer now changed nothing: the conversation keeps
    // the plan it was told, and a later refresh reads it again.
    let Ok(mut page) = link.plan_read(&scope.account, whose.resident.clone()).await else { return Ok(()) };
    // Another's is not this one's to see, nor its version this one's to
    // write over.
    if link.plan_of_another(&page, &whose).is_some() {
        page = Page { owner: page.owner, metadata: None, body: None };
    } else {
        crate::plan_file::pulled(node, id, &page);
    }
    let version = serde_json::to_string(&Seen { account: scope, owner: page.owner.clone(), version: page.version() })
        .map_err(|error| Failure::new(error.to_string()))?;
    crate::context::plan(node, id, &page, &version).map_err(|error| Failure::new(error.to_string()))?;
    node.plan_reads.done(id, generation);
    Ok(())
}

/// Writes `markdown`, what the plan file of the conversation `id` holds
/// now, as the plan's next version, written by the conversation. A version
/// another wrote meanwhile is written over: the file is the plan. None where
/// no hub keeps plans for it.
pub(crate) async fn save(node: &Arc<Node>, id: &ResidentId, markdown: String) -> Result<Option<Page>, Failure> {
    if !crate::plan_file::allowed(node, id)? {
        return Err(Failure::not_run("this conversation has no plan file on this machine"));
    }
    let (about, _) = node.residents.about(id).ok_or_else(|| Failure::refused(words::RESIDENT))?;
    if matches!(about.origin, Origin::Colleague { .. }) {
        return Ok(None);
    }
    let whose = Address { machine: about.machine.clone(), resident: id.clone() };
    let previous = seen(node, id)?;
    let selected = previous.as_ref().map(|seen| seen.account.clone()).or_else(|| only_account(node, &about.acting));
    let Some(scope) = selected else { return Ok(None) };
    let Some(link) = node.link_of(&scope.hub) else { return Ok(None) };
    if !link.view().conversation_plans {
        return Ok(None);
    }
    for _ in 0..3 {
        // The plan kept under this short name may be that of a conversation
        // of the same one on another machine, and become so between two
        // tries: each try looks, and writes only over the version it saw.
        let current = link.plan_read(&scope.account, id.clone()).await.map_err(Failure::new)?;
        if let Some(other) = link.plan_of_another(&current, &whose) {
            return Err(Failure::not_run(words::elsewhere(other)));
        }
        let author = Author::Resident { address: whose.clone() };
        match link
            .plan_write(&scope.account, id.clone(), current.version(), markdown.clone(), author, None)
            .await
            .map_err(Failure::new)?
        {
            plan::Result::Page { page } => {
                let version = serde_json::to_string(&Seen {
                    account: scope.clone(),
                    owner: page.owner.clone(),
                    version: page.version(),
                })
                .map_err(|error| Failure::new(error.to_string()))?;
                crate::context::plan(node, id, &page, &version).map_err(|error| Failure::new(error.to_string()))?;
                remember(node, id, scope, &page)?;
                return Ok(Some(page));
            }
            plan::Result::Conflict { .. } => {}
        }
    }
    Err(Failure::new(words::CONFLICT))
}

pub(crate) fn file_account(node: &Node, id: &ResidentId, acting: &Acting) -> Result<Option<HubAccount>, Failure> {
    crate::placement::account(node, acting, seen(node, id)?.map(|seen| seen.account))
}

/// The one account `acting` works for, where it is one.
fn only_account(node: &Node, acting: &Acting) -> Option<HubAccount> {
    crate::placement::account(node, acting, None).ok().flatten()
}

pub(crate) fn context_seen(node: &Node, id: &ResidentId, version: &str) -> Result<(), Failure> {
    let observed: Seen = serde_json::from_str(version).map_err(|error| Failure::new(error.to_string()))?;
    keep_seen(node, id, &observed)
}

/// Before the identity record goes, remove the owner's plan at every account it
/// can belong to. Failure retains the existing durable conversation-deletion intent.
pub(crate) async fn delete(node: &Arc<Node>, about: &About) -> Result<(), Failure> {
    if matches!(about.origin, Origin::Colleague { .. }) {
        return Ok(());
    }
    let mut scopes = match &about.acting {
        Acting::Accounts { accounts } => accounts.clone(),
        Acting::Here => node
            .links()
            .into_iter()
            .flat_map(|link| {
                node.accounts_on(&link, &Acting::Here)
                    .into_iter()
                    .map(move |account| HubAccount { hub: link.membership().id.clone(), account })
            })
            .collect(),
    };
    if let Some(seen) = seen(node, &about.id)?
        && !scopes.contains(&seen.account)
    {
        scopes.push(seen.account);
    }
    for scope in scopes {
        let link = node.link_of(&scope.hub).ok_or_else(|| Failure::new(words::ROUTE))?;
        link.refresh().await.map_err(Failure::new)?;
        // Earlier hubs never held plans; their conversations retain their former deletion behavior.
        if !link.view().conversation_plans {
            continue;
        }
        // Another conversation's of the same short name stays.
        let page = link.plan_read(&scope.account, about.id.clone()).await.map_err(Failure::new)?;
        if link
            .plan_of_another(&page, &Address { machine: about.machine.clone(), resident: about.id.clone() })
            .is_some()
        {
            continue;
        }
        link.plan_delete(&scope.account, &about.id).await.map_err(Failure::new)?;
    }
    Ok(())
}

/// Generic record calls must not turn a colleague's read-only plan into a write path.
pub(crate) fn records_allowed(
    node: &Node,
    caller: &Caller,
    request: &fastctx_types::account_records::Request,
) -> Result<(), Failure> {
    use fastctx_types::account_records::Request;
    // A goal changes only through the goal command's restricted state entry; a
    // resident or page writing the record directly would bypass that (the goal
    // runtime's own provider reaches the record below this gate, not through it).
    if [crate::goals::operations::KIND, crate::goals::past::KIND].contains(&request.kind())
        && matches!(request, Request::Write { .. } | Request::Delete { .. })
        && matches!(caller, Caller::Resident(_) | Caller::Page(_))
    {
        return Err(Failure::refused(fastctx_types::goals::words::DIRECT_RECORD));
    }
    // Sentinels and their settings change only through the sentinel command and
    // the user's settings; their home is the account's word that a machine may act for it.
    if [fastctx_sentinel::lifecycle::KIND, fastctx_types::sentinel_settings::RECORD_KIND].contains(&request.kind())
        && matches!(request, Request::Write { .. } | Request::Delete { .. })
        && matches!(caller, Caller::Resident(_) | Caller::Page(_))
    {
        return Err(Failure::refused(fastctx_sentinel::words::DIRECT_RECORD));
    }
    // Skills are the user's to write: a session reads them, never changes them.
    if [fastctx_types::skills::KIND, fastctx_types::skills::INDEX_KIND].contains(&request.kind())
        && matches!(request, Request::Write { .. } | Request::Delete { .. })
        && matches!(caller, Caller::Resident(_) | Caller::Page(_))
    {
        return Err(Failure::refused(fastctx_types::skills::words::USER_ONLY));
    }
    if request.kind() != plan::KIND || matches!(request, Request::Read { .. } | Request::List { .. }) {
        return Ok(());
    }
    if let Caller::Resident(address) = caller {
        if !node.is_me(&address.machine) {
            return Err(Failure::refused(words::RESIDENT));
        }
        if node
            .residents
            .about(&address.resident)
            .is_some_and(|(about, _)| matches!(about.origin, Origin::Colleague { .. }))
        {
            return Err(Failure::refused(words::READ_ONLY));
        }
    }
    Ok(())
}
