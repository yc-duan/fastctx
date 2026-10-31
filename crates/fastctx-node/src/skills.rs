//! The account's skills as its sessions have them (`fastctx_types::skills`,
//! `world-user-config.md`, 技能): the names and sentences of the skills of
//! the one account a conversation works for, in its "skills" context, brought
//! up to date as the user changes them; and a skill read whole, by whoever
//! works for that account and no one else.

mod following;

pub(crate) use following::follow;

use std::sync::Arc;

use fastctx_net::link::Link;
use fastctx_types::AccountName;
use fastctx_types::access::Acting;
use fastctx_types::account_records::{Request, Response};
use fastctx_types::ipc::Failure;
use fastctx_types::skills::{self, Index, Skill};

pub(crate) mod cache;
pub(crate) mod copies;
mod location;
use crate::Node;
use crate::town::caller::Caller;
use location::locate;

/// The one account `acting` works for, with its hub's link: a conversation
/// of no account, or of several, has no skills.
fn account_of(node: &Node, acting: &Acting) -> Option<(Arc<Link>, AccountName)> {
    let Acting::Accounts { accounts } = acting else { return None };
    let [account] = accounts.as_slice() else { return None };
    node.link_of(&account.hub).map(|link| (link, account.account.clone()))
}

/// The index of `account`'s skills, as its hub keeps it.
async fn index(link: &Link, account: &AccountName) -> Result<(Index, String), String> {
    let read = Request::Read { kind: skills::INDEX_KIND.into(), key: skills::INDEX_KEY.into() };
    match link.account_records(account, read).await? {
        Response::Read { record: Some(record) } if !record.metadata.deleted => Ok((
            serde_json::from_value(record.body).map_err(|error| format!("reading the skills' index: {error}"))?,
            record.metadata.version.to_string(),
        )),
        Response::Read { .. } => Ok((Index::default(), String::new())),
        other => Err(format!("the hub answered reading the skills with {other:?}")),
    }
}

/// What a conversation working for `acting` is told of its skills; `None`
/// when its hub cannot be asked now, and it keeps what it was told.
pub(crate) async fn context(node: &Node, acting: &Acting) -> Option<(String, String)> {
    let Some((link, account)) = account_of(node, acting) else { return Some((String::new(), String::new())) };
    // An account its hub no longer has keeps no skills; its hub is not asked.
    if crate::authorization::current_view(&link).is_ok_and(|view| view.account(&account).is_none()) {
        return Some((String::new(), String::new()));
    }
    match cache::get(node, &link, &account).await {
        Ok(body) => Some(body),
        Err(failed) => {
            if failed.news {
                node.log.line(format_args!("reading the skills of {account}: {}", failed.error));
            }
            None
        }
    }
}

/// The skill `name` of the account `caller` works for, whole.
pub(crate) async fn read(
    node: &Arc<Node>,
    caller: &Caller,
    name: &str,
) -> Result<(Skill, Option<fastctx_types::placement::Location>), Failure> {
    if !skills::valid_name(name) {
        return Err(Failure::not_run(skills::words::missing(name)));
    }
    let (link, account) = crate::profile::scope(node, caller, None, None)?;
    if link.view().skill_files {
        return locate(node, caller, &link, &account, name).await;
    }
    let read = Request::Read { kind: skills::KIND.into(), key: name.into() };
    match link.account_records(&account, read).await.map_err(Failure::new)? {
        Response::Read { record: Some(record) } if !record.metadata.deleted => serde_json::from_value(record.body)
            .map(|skill| (skill, None))
            .map_err(|error| Failure::new(format!("reading the skill {name}: {error}"))),
        Response::Read { .. } => Err(Failure::not_run(skills::words::missing(name))),
        other => Err(Failure::new(format!("the hub answered reading the skill {name} with {other:?}"))),
    }
}
