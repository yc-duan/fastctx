//! One coalescing worker per hub; a changed document never downloads its peers.

use crate::Node;
use fastctx_types::{
    AccountName,
    access::{HubAccount, HubId, TownId},
    account_records::Changed,
    replicas::Head,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub(crate) struct Updates(Mutex<BTreeMap<HubId, Pending>>);
#[derive(Default)]
struct Pending {
    all: bool,
    projects: BTreeSet<TownId>,
    topics: BTreeMap<(TownId, String), u64>,
    records: BTreeMap<(AccountName, String, String), Changed>,
    /// Documents a full synchronization could not bring up to date, by copy.
    documents: BTreeMap<String, (HubAccount, Head)>,
    wake: Arc<tokio::sync::Notify>,
}

pub(crate) enum Change {
    All,
    Project(TownId),
    History { town: TownId, name: String, version: u64 },
    Record(Changed),
}

/// Gives the documents a synchronization of `account` could not bring up to
/// date to this hub's worker, to try again on their own. What the first
/// failure was, if any.
pub(crate) fn retry_later(node: &Arc<Node>, account: &HubAccount, failed: Vec<(Head, String)>) -> Option<String> {
    let (_, first) = failed.first()?;
    let error = format!("{} documents not synchronized; the first: {first}", failed.len());
    start(node, &account.hub, false, |pending| {
        for (head, _) in failed {
            pending.documents.insert(super::state::identity(account, &head), (account.clone(), head));
        }
    });
    Some(error)
}

pub(crate) fn schedule(node: &Arc<Node>, hub: &HubId, change: Change) {
    start(node, hub, true, |pending| match change {
        Change::All => pending.all = true,
        Change::Project(project) => {
            pending.projects.insert(project);
        }
        Change::History { town, name, version } => {
            pending.topics.entry((town, name)).and_modify(|kept| *kept = (*kept).max(version)).or_insert(version);
        }
        Change::Record(change) => {
            pending
                .records
                .insert((change.account.clone(), change.metadata.kind.clone(), change.metadata.key.clone()), change);
        }
    });
}

/// Adds to the hub's pending work, starting its worker when none runs. A
/// running worker is woken only for news, not for work given back to it.
fn start(node: &Arc<Node>, hub: &HubId, news: bool, add: impl FnOnce(&mut Pending)) {
    let wake = {
        let mut updates = node.replicas.updates.0.lock().expect("never poisoned");
        let running = updates.contains_key(hub);
        let pending = updates.entry(hub.clone()).or_default();
        add(pending);
        if running {
            if news {
                pending.wake.notify_one();
            }
            return;
        }
        pending.wake.clone()
    };
    let (node, hub) = (node.clone(), hub.clone());
    fastctx_platform::task::spawn(async move { run(node, hub, wake).await });
}

async fn run(node: Arc<Node>, hub: HubId, wake: Arc<tokio::sync::Notify>) {
    let mut again = crate::again::Again::default();
    let mut previous = None;
    loop {
        let Some(link) = node.link_of(&hub) else {
            node.replicas.updates.0.lock().expect("never poisoned").remove(&hub);
            return;
        };
        let (all, projects, topics, records, documents) = {
            let mut updates = node.replicas.updates.0.lock().expect("never poisoned");
            let pending = updates.get_mut(&hub).expect("one worker owns the pending entry");
            if !pending.all
                && pending.projects.is_empty()
                && pending.topics.is_empty()
                && pending.records.is_empty()
                && pending.documents.is_empty()
            {
                updates.remove(&hub);
                return;
            }
            (
                std::mem::take(&mut pending.all),
                std::mem::take(&mut pending.projects),
                std::mem::take(&mut pending.topics),
                std::mem::take(&mut pending.records),
                std::mem::take(&mut pending.documents),
            )
        };
        let mut retry = Pending::default();
        let mut first = None;
        if all && let Err(error) = super::sync::all(&node, &link).await {
            retry.all = true;
            first = Some(error);
        }
        if !all || retry.all {
            for project in projects {
                if let Err(error) = super::sync::project(&node, &link, &project).await {
                    retry.projects.insert(project);
                    first = first.or(Some(error));
                }
            }
            for ((town, name), version) in topics {
                if let Err(error) = super::sync::topic(&node, &link, &town, &name, version).await {
                    retry.topics.insert((town, name), version);
                    first = first.or(Some(error));
                }
            }
            for (key, record) in records {
                if let Err(error) = super::sync::record(&node, &link, &record).await {
                    retry.records.insert(key, record);
                    first = first.or(Some(error));
                }
            }
        }
        for (id, (account, head)) in documents {
            if let Err(error) = super::sync::apply(&node, &link, &account, head.clone()).await {
                retry.documents.insert(id, (account, head));
                first = first.or(Some(error));
            }
        }
        let Some(error) = first else {
            previous = None;
            again.reset();
            continue;
        };
        if previous.as_ref() != Some(&error) {
            node.log.line(format_args!("synchronizing document replicas: {error}"));
            previous = Some(error);
        }
        {
            let mut updates = node.replicas.updates.0.lock().expect("never poisoned");
            let pending = updates.get_mut(&hub).expect("worker entry");
            pending.all |= retry.all;
            pending.projects.extend(retry.projects);
            for (key, version) in retry.topics {
                pending.topics.entry(key).and_modify(|kept| *kept = (*kept).max(version)).or_insert(version);
            }
            for (key, old) in retry.records {
                pending.records.entry(key).or_insert(old);
            }
            for (key, document) in retry.documents {
                pending.documents.entry(key).or_insert(document);
            }
        }
        let mut ways = node.ways.subscribe();
        tokio::select! { () = tokio::time::sleep(again.next()) => {}, () = wake.notified() => {}, _ = ways.changed() => {} }
    }
}
