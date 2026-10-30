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

use crate::Node;
use crate::town::caller::Caller;

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
