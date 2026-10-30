//! Each invalidated account's skill index is read once for all its conversations.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fastctx_types::access::{Acting, HubId};
use fastctx_types::account_records::Changed;
use fastctx_types::{AccountName, ResidentId, skills};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

use crate::Node;

#[derive(Default)]
struct Pending {
    all: bool,
    accounts: BTreeSet<AccountName>,
}

impl Pending {
    fn heard(&mut self, change: Option<Changed>) {
        match change {
            None => self.all = true,
            Some(change) if change.metadata.kind == skills::INDEX_KIND => {
                self.accounts.insert(change.account);
            }
            Some(_) => {}
        }
    }

    fn empty(&self) -> bool {
        !self.all && self.accounts.is_empty()
    }
}

pub(crate) fn follow(node: &Arc<Node>) {
    let node = node.clone();
    let followed: Arc<Mutex<HashSet<HubId>>> = Arc::default();
    tokio::spawn(async move {
        loop {
            for link in node.links() {
                let hub = link.hub().clone();
                if !followed.lock().expect("never poisoned").insert(hub.clone()) {
                    continue;
                }
                let (node, followed) = (node.clone(), followed.clone());
                let mut changes = link.record_changes();
                tokio::spawn(async move {
                    let mut pending = Pending { all: true, ..Default::default() };
                    let mut again = crate::again::Again::default();
                    let mut errors = BTreeMap::new();
                    'following: loop {
                        if pending.empty() {
                            match changes.recv().await {
                                Ok(change) => pending.heard(change),
                                Err(RecvError::Lagged(_)) => pending.all = true,
                                Err(RecvError::Closed) => break,
                            }
                        }
                        loop {
                            match changes.try_recv() {
                                Ok(change) => pending.heard(change),
                                Err(TryRecvError::Lagged(_)) => pending.all = true,
                                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                            }
                        }
                        if pending.empty() {
                            continue;
                        }
                        pending.accounts = tell(&node, &hub, std::mem::take(&mut pending), &mut errors).await;
                        if pending.empty() {
                            again = crate::again::Again::default();
                            continue;
                        }
                        let wait = tokio::time::sleep(again.next());
                        tokio::pin!(wait);
                        loop {
                            tokio::select! {
                                () = &mut wait => break,
                                change = changes.recv() => match change {
                                    Ok(change) => {
                                        if change.as_ref().is_none_or(|changed| changed.metadata.kind == skills::INDEX_KIND) {
                                            pending.heard(change);
                                            break;
                                        }
                                    }
                                    Err(RecvError::Lagged(_)) => { pending.all = true; break; }
                                    Err(RecvError::Closed) => break 'following,
                                },
                            }
                        }
                    }
                    followed.lock().expect("never poisoned").remove(&hub);
                });
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

async fn tell(
    node: &Arc<Node>,
    hub: &HubId,
    pending: Pending,
    errors: &mut BTreeMap<AccountName, String>,
) -> BTreeSet<AccountName> {
    let mut retry = BTreeSet::new();
    let Some(link) = node.link_of(hub) else { return retry };
    // What was kept of them is stale now, for every conversation's read.
    if pending.all {
        node.skill_index.invalidate(hub, None);
    } else {
        for account in &pending.accounts {
            node.skill_index.invalidate(hub, Some(account));
        }
    }
    let mut conversations: BTreeMap<AccountName, Vec<ResidentId>> = BTreeMap::new();
    for resident in node.residents.everyone().filter(|resident| resident.removed.is_none()) {
        let Acting::Accounts { accounts } = &resident.about.acting else { continue };
        let [one] = accounts.as_slice() else { continue };
        if one.hub == *hub && (pending.all || pending.accounts.contains(&one.account)) {
            conversations.entry(one.account.clone()).or_default().push(resident.id);
        }
    }
    for (account, residents) in conversations {
        let (index, version) = match super::index(&link, &account).await {
            Ok(read) => read,
            Err(error) => {
                if errors.get(&account) != Some(&error) {
                    node.log.line(format_args!("reading the skills of {account}: {error}"));
                    errors.insert(account.clone(), error);
                }
                retry.insert(account);
                continue;
            }
        };
        let (writing, text) = (node.clone(), skills::context(&index));
        let acting = Acting::one(hub.clone(), account.clone());
        let written = tokio::task::spawn_blocking(move || {
            let mut failed = Vec::new();
            for id in residents {
                if !writing.residents.about(&id).is_some_and(|(about, _)| about.acting == acting) {
                    continue;
                }
                if let Err(error) = crate::context::sources::set_skills(&writing, &id, &text, &version)
                    && writing.residents.active(&id).is_ok()
                {
                    failed.push(format!("{id}: {}", error.message));
                }
            }
            failed
        })
        .await;
        let error = match written {
            Ok(failed) if failed.is_empty() => None,
            Ok(failed) => Some(format!("{} conversations: {}", failed.len(), failed[0])),
            Err(error) => Some(error.to_string()),
        };
        if let Some(error) = error {
            if errors.get(&account) != Some(&error) {
                node.log.line(format_args!("telling {account}'s conversations their skills: {error}"));
                errors.insert(account.clone(), error);
            }
            retry.insert(account);
        } else {
            errors.remove(&account);
        }
    }
    retry
}
