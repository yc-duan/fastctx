//! The account's skills as its sessions have them (`fastctx_types::skills`,
//! `world-user-config.md`, 技能): the names and sentences of the skills of
//! the one account a conversation works for, in its "skills" context, brought
//! up to date as the user changes them; and a skill read whole, by whoever
//! works for that account and no one else.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fastctx_net::link::Link;
use fastctx_types::AccountName;
use fastctx_types::access::{Acting, HubId};
use fastctx_types::account_records::{Request, Response};
use fastctx_types::ipc::Failure;
use fastctx_types::skills::{self, Index, Skill};
use tokio::sync::broadcast::error::RecvError;

use crate::Node;
use crate::town::caller::Caller;

/// How often the networks this computer is on are looked at for one whose
/// skills are not followed yet.
const LOOK: Duration = Duration::from_secs(10);

/// The one account `acting` works for, with its hub's link: a conversation
/// of no account, or of several, has no skills.
fn account_of(node: &Node, acting: &Acting) -> Option<(Arc<Link>, AccountName)> {
    let Acting::Accounts { accounts } = acting else { return None };
    let [account] = accounts.as_slice() else { return None };
    node.link_of(&account.hub).map(|link| (link, account.account.clone()))
}

/// The index of `account`'s skills, as its hub keeps it.
async fn index(link: &Link, account: &AccountName) -> Result<Index, String> {
    let read = Request::Read { kind: skills::INDEX_KIND.into(), key: skills::INDEX_KEY.into() };
    match link.account_records(account, read).await? {
        Response::Read { record: Some(record) } if !record.metadata.deleted => {
            serde_json::from_value(record.body).map_err(|error| format!("reading the skills' index: {error}"))
        }
        Response::Read { .. } => Ok(Index::default()),
        other => Err(format!("the hub answered reading the skills with {other:?}")),
    }
}

/// What a conversation working for `acting` is told of its skills; `None`
/// when its hub cannot be asked now, and it keeps what it was told.
pub(crate) async fn context(node: &Node, acting: &Acting) -> Option<String> {
    let Some((link, account)) = account_of(node, acting) else { return Some(String::new()) };
    match index(&link, &account).await {
        Ok(index) => Some(skills::context(&index)),
        Err(error) => {
            node.log.line(format_args!("reading the skills of {account}: {error}"));
            None
        }
    }
}

/// The skill `name` of the account `caller` works for, whole.
pub(crate) async fn read(node: &Node, caller: &Caller, name: &str) -> Result<Skill, Failure> {
    if !skills::valid_name(name) {
        return Err(Failure::not_run(skills::words::missing(name)));
    }
    let (link, account) = crate::profile::scope(node, caller, None, None)?;
    let read = Request::Read { kind: skills::KIND.into(), key: name.into() };
    match link.account_records(&account, read).await.map_err(Failure::new)? {
        Response::Read { record: Some(record) } if !record.metadata.deleted => serde_json::from_value(record.body)
            .map_err(|error| Failure::new(format!("reading the skill {name}: {error}"))),
        Response::Read { .. } => Err(Failure::not_run(skills::words::missing(name))),
        other => Err(Failure::new(format!("the hub answered reading the skill {name} with {other:?}"))),
    }
}

/// Follows the skills of the accounts this computer works for on each of
/// its networks: as the user changes an account's, every conversation here
/// working for that account is told the new ones.
pub(crate) fn follow(node: &Arc<Node>) {
    let node = node.clone();
    let followed: Arc<Mutex<HashSet<HubId>>> = Arc::default();
    tokio::spawn(async move {
        loop {
            for link in node.links() {
                let hub = link.membership().id.clone();
                if !followed.lock().expect("never poisoned").insert(hub.clone()) {
                    continue;
                }
                let (node, followed) = (node.clone(), followed.clone());
                let mut changes = link.record_changes();
                tokio::spawn(async move {
                    loop {
                        match changes.recv().await {
                            Ok(Some(changed)) if changed.metadata.kind == skills::INDEX_KIND => {
                                tell(&node, &hub, Some(&changed.account)).await;
                            }
                            Ok(Some(_)) => {}
                            // Hints were missed: every account's are told again.
                            Ok(None) | Err(RecvError::Lagged(_)) => tell(&node, &hub, None).await,
                            Err(RecvError::Closed) => break,
                        }
                    }
                    // The network was left: followed again should it come back.
                    followed.lock().expect("never poisoned").remove(&hub);
                });
            }
            tokio::time::sleep(LOOK).await;
        }
    });
}

/// Tells the conversations here working for `account` on `hub`, or for any
/// account there, their skills as they are now.
async fn tell(node: &Arc<Node>, hub: &HubId, account: Option<&AccountName>) {
    for resident in node.residents.everyone() {
        if resident.removed.is_some() {
            continue;
        }
        let Acting::Accounts { accounts } = &resident.about.acting else { continue };
        let [one] = accounts.as_slice() else { continue };
        if one.hub != *hub || account.is_some_and(|account| *account != one.account) {
            continue;
        }
        if let Err(error) = crate::context::sources::skills(node, &resident.id, &resident.about.acting).await {
            node.log.line(format_args!("telling {} its skills: {}", resident.id, error.message));
        }
    }
}
