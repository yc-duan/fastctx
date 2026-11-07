//! One resumable exporter per node, woken by connections and history requests.

use super::{archive, state};
use crate::Node;
use fastctx_types::{
    Address, ResidentId,
    history::Scope,
    owned_records::{Request, Response},
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Default)]
pub(super) struct Work {
    running: bool,
    requested: bool,
}

pub(crate) fn schedule(node: &Arc<Node>) {
    if node.links().is_empty() {
        return;
    }
    let mut work = node.history_imports.work.lock().expect("never poisoned");
    work.requested = true;
    node.history_imports.wake.notify_one();
    if work.running {
        return;
    }
    work.running = true;
    let node = node.clone();
    fastctx_platform::task::spawn(async move { run(node).await });
}

async fn run(node: Arc<Node>) {
    let mut again = crate::again::Again::default();
    let mut last = None;
    loop {
        node.history_imports.work.lock().expect("never poisoned").requested = false;
        let mut errors = Vec::new();
        for resident in node.residents.everyone().filter(|resident| resident.removed.is_none()) {
            if let Err(error) = one(&node, &resident.id).await {
                errors.push(format!("{}: {error}", resident.id));
            }
        }
        let error = (!errors.is_empty()).then(|| errors.join("; "));
        if error != last {
            if let Some(error) = &error {
                node.log.line(format_args!("importing standalone history: {error}"));
            }
            last = error.clone();
        }
        {
            let mut work = node.history_imports.work.lock().expect("never poisoned");
            if node.links().is_empty() || (error.is_none() && !work.requested) {
                work.running = false;
                return;
            }
            if work.requested {
                continue;
            }
        }
        let mut ways = node.ways.subscribe();
        tokio::select! {
            () = tokio::time::sleep(again.next()) => {},
            () = node.history_imports.wake.notified() => {},
            _ = ways.changed() => {},
        }
    }
}

async fn one(node: &Arc<Node>, resident: &ResidentId) -> Result<(), String> {
    let owner = Address { resident: resident.clone(), machine: node.machine() };
    let scope = Scope::Conversation { resident: resident.clone(), machine: owner.machine.clone() };
    let records = super::super::local::directory(node, &scope);
    if !super::pending(node, &scope) {
        return Ok(());
    }
    let (mut transfer, inventory) = {
        let _history = super::lock(node, &scope).await;
        let (about, _) = node.residents.about(resident).ok_or("the conversation was removed")?;
        let inventory = state::inventory(&records).map_err(|error| error.to_string())?;
        let transfer = match state::read(&records).map_err(|error| error.to_string())? {
            Some(transfer) => transfer,
            None => {
                let account = crate::placement::account(node, &about.acting, None)
                    .map_err(|error| error.message)?
                    .ok_or("the standalone history needs an unambiguous importing account")?;
                let link = node.link_of(&account.hub).ok_or("the importing hub is unavailable")?;
                let opened =
                    link.view().account(&account.account).ok_or("the importing account is unavailable")?.opened;
                let sources = inventory
                    .values()
                    .flatten()
                    .map(|(_, path)| archive::Original::of(node.world.path(), resident.as_str(), path))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| error.to_string())?;
                let transfer = state::Transfer {
                    account,
                    opened,
                    names: BTreeMap::new(),
                    sources,
                    conversation: None,
                    skipped: BTreeMap::new(),
                    originals: None,
                    done: false,
                };
                state::keep(&records, &transfer).map_err(|error| error.to_string())?;
                transfer
            }
        };
        if !transfer.done && transfer.originals.is_none() {
            for source in &transfer.sources {
                source.verify().map_err(|error| error.to_string())?;
            }
        }
        (transfer, inventory)
    };
    if transfer.done {
        return Ok(());
    }
    let link = node.link_of(&transfer.account.hub).ok_or("the importing hub is unavailable")?;
    let authorized = || -> Result<(), String> {
        let (about, _) = node.residents.about(resident).ok_or("the conversation was removed")?;
        if !node.serves(&about.acting, &transfer.account.hub, &transfer.account.account)
            || link.view().account(&transfer.account.account).is_none_or(|account| account.opened != transfer.opened)
        {
            return Err("the history's importing account changed or was replaced".into());
        }
        Ok(())
    };
    authorized()?;
    if transfer.originals.is_none() {
        let (about, _) = node.residents.about(resident).ok_or("the conversation was removed")?;
        crate::record_ownership::register_at(node, resident, &about.acting, &transfer.account)
            .await
            .map_err(|error| error.message)?;
        let Response::Conversation { conversation } =
            link.owned_records(&transfer.account.account, Request::Resolve { conversation: owner.clone() }).await?
        else {
            return Err("the conversation's import ownership was not confirmed".into());
        };
        if conversation.account != transfer.account.account {
            return Err("the conversation belongs to another account".into());
        }
        transfer.conversation = Some(conversation.clone());
        state::keep(&records, &transfer).map_err(|error| error.to_string())?;
        if !super::settling(node, &fastctx_types::replicas::Scope::Conversation { conversation }) {
            return Ok(());
        }
        let mut known = link
            .conversation_history_catalog(&transfer.account.account, &owner)
            .await?
            .documents
            .into_iter()
            .map(|revision| revision.name)
            .collect();
        let mut errors = Vec::new();
        for (name, versions) in &inventory {
            if let Err(error) = super::replay::topic(
                node,
                &link,
                &owner,
                &records,
                &mut transfer,
                name,
                versions,
                &inventory,
                &mut known,
            )
            .await
            {
                errors.push(format!("{name}: {error}"));
            }
        }
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
    }
    let _history = super::lock(node, &scope).await;
    let (about, _) = node.residents.about(resident).ok_or("the conversation was removed")?;
    if !node.serves(&about.acting, &transfer.account.hub, &transfer.account.account)
        || link.view().account(&transfer.account.account).is_none_or(|account| account.opened != transfer.opened)
    {
        return Err("the history's importing account changed before archival".into());
    }
    if transfer.originals.is_none() {
        if state::inventory(&records).map_err(|error| error.to_string())? != inventory {
            return Err("the standalone history changed during import; retaining its originals".into());
        }
        for source in &transfer.sources {
            source.verify().map_err(|error| error.to_string())?;
        }
        transfer.originals = Some(
            transfer
                .sources
                .iter()
                .filter(|source| {
                    source
                        .source()
                        .parent()
                        .and_then(std::path::Path::file_name)
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| !transfer.skipped.contains_key(name))
                })
                .cloned()
                .collect(),
        );
        state::keep(&records, &transfer).map_err(|error| error.to_string())?;
    }
    state::finish(&records, &mut transfer).map_err(|error| error.to_string())?;
    super::pending(node, &scope);
    crate::context::invalidations::history(node, &scope);
    crate::replicas::updates::schedule(node, &transfer.account.hub, crate::replicas::updates::Change::All);
    node.memory_changed();
    Ok(())
}
