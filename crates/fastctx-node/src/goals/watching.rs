//! Account changes are observed at the conversation's home, including changes made by its remote jobs.

use super::{operations, records::Changed, scope};
use crate::Node;
use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::ipc::Failure;
use fastctx_types::{Address, ResidentId};
use std::collections::BTreeSet;
use std::sync::Arc;

pub(crate) fn start(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        let mut views = node.views.subscribe();
        let mut residents = node.residents.changes();
        loop {
            let mut accounts = BTreeSet::new();
            for link in node.links() {
                for account in node.accounts_on(&link, &Acting::Here) {
                    accounts.insert(HubAccount { hub: link.membership().id.clone(), account });
                }
            }
            for record in node.residents.everyone() {
                if let Acting::Accounts { accounts: serving } = record.about.acting {
                    accounts.extend(serving);
                }
            }
            for account in accounts {
                if node.goals.watching.lock().expect("goal watches").insert(account.clone()) {
                    let node = node.clone();
                    super::archive::start(&node, &account);
                    tokio::spawn(async move { follow(node, account).await });
                }
            }
            tokio::select! {
                result = views.changed() => { if result.is_err() { return; } },
                result = residents.changed() => { if result.is_err() { return; } },
            }
        }
    });
}

async fn follow(node: Arc<Node>, account: HubAccount) {
    let mut again = crate::again::Again::default();
    let mut previous = None;
    loop {
        let result = consume(&node, &account).await;
        if let Err(error) = result
            && previous.as_ref() != Some(&error.message)
        {
            node.log.line(format_args!("watching account goals: {}", error.message));
            previous = Some(error.message);
        }
        tokio::time::sleep(again.next()).await;
    }
}

async fn consume(node: &Arc<Node>, account: &HubAccount) -> Result<(), Failure> {
    let mut changes = operations::bounded(node.goals.records.watch(account, operations::KIND)).await?;
    let rows = operations::bounded(node.goals.records.list(account, operations::KIND)).await?;
    for row in rows {
        refresh_one(node, account, row).await;
    }
    let pinned: Vec<_> = node.residents.everyone().map(|record| record.about.id).collect();
    for resident in pinned {
        if scope::pinned(node, &resident)?.as_ref() == Some(account) {
            refresh_one(node, account, Changed { key: resident.to_string(), version: 0 }).await;
        }
    }
    loop {
        for row in changes.next().await? {
            refresh_one(node, account, row).await;
        }
    }
}

/// One conversation's trouble stays its own: the account's other goals go
/// on, and its goal is read again apart, with its own waits, until the
/// account's version of it is in place; no later change need come for it.
async fn refresh_one(node: &Arc<Node>, account: &HubAccount, row: Changed) {
    let key = row.key.clone();
    if let Err(failure) = refresh(node, account, row).await {
        node.log.line(format_args!("goal of {key}: {}; reading it again", failure.message));
        retry(node, account, key);
    }
}

fn retry(node: &Arc<Node>, account: &HubAccount, key: String) {
    if !node.goals.retrying.lock().expect("goal retries").insert((account.clone(), key.clone())) {
        return;
    }
    let (node, account) = (node.clone(), account.clone());
    tokio::spawn(async move {
        let mut again = crate::again::Again::default();
        let mut told = None;
        loop {
            tokio::time::sleep(again.next()).await;
            match refresh(&node, &account, Changed { key: key.clone(), version: 0 }).await {
                Ok(()) => break,
                Err(failure) if told.as_ref() != Some(&failure.message) => {
                    node.log.line(format_args!("goal of {key}: {}; reading it again", failure.message));
                    told = Some(failure.message);
                }
                Err(_) => {}
            }
        }
        node.goals.retrying.lock().expect("goal retries").remove(&(account, key));
    });
}

async fn refresh(node: &Arc<Node>, account: &HubAccount, row: Changed) -> Result<(), Failure> {
    let Ok(resident) = row.key.parse::<ResidentId>() else { return Ok(()) };
    if !node.residents.knows(&resident) {
        return Ok(());
    }
    if scope::pinned(node, &resident)?.is_some_and(|pinned| &pinned != account) {
        return Ok(());
    }
    if row.version != 0
        && node
            .goals
            .views
            .lock()
            .expect("goal views")
            .get(&resident)
            .is_some_and(|view| &view.account == account && view.version.is_some_and(|version| version >= row.version))
    {
        return Ok(());
    }
    let gate = crate::control_queue::gate(node, &resident);
    let _action = gate.lock().await;
    let _place = node.places.using(resident.as_str()).await;
    let Some((about, _)) = node.residents.about(&resident) else { return Ok(()) };
    if !node.serves(&about.acting, &account.hub, &account.account)
        || scope::pinned(node, &resident)?.is_some_and(|pinned| &pinned != account)
    {
        return Ok(());
    }
    let address = Address { machine: node.machine(), resident: resident.clone() };
    let view = operations::read(node, &address, account).await?;
    if view.goal.is_none() && scope::pinned(node, &resident)?.is_none() {
        return Ok(());
    }
    scope::keep(node, &resident, account).map_err(|error| Failure::new(error.to_string()))?;
    let previous = node
        .goals
        .views
        .lock()
        .expect("goal views")
        .get(&resident)
        .and_then(|view| view.goal.as_ref())
        .map(|goal| goal.id.clone());
    if view.goal.is_none() || previous.is_some_and(|previous| view.goal.as_ref().is_none_or(|goal| goal.id != previous))
    {
        super::archive::for_resident(node, account, &resident).await?;
    }
    operations::apply(node, &resident, &view).await
}
