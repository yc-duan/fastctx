//! Connects the goal runtime to the account records and named context the
//! records line provides: a goal is a `goal`-kind account record keyed by its
//! conversation, and its live state rides a "goal" context block.

use std::sync::{Arc, Weak};

use fastctx_net::link::Link;
use fastctx_net::link::account_records::RecordWatch;
use fastctx_types::account_records::{Request, Response, Watched};
use fastctx_types::access::HubAccount;
use fastctx_types::ipc::Failure;
use fastctx_types::ResidentId;
use serde_json::Value;

use super::context::Context;
use super::records::{Changed, Changes, Exchanged, Record, Records, Reply};
use crate::Node;

const GOAL_CONTEXT_CHANGED: &str = "The goal changed; this replaces the previous goal in your context.";

/// Account records reached through this machine's link to the account's hub.
struct AccountRecords {
    node: Weak<Node>,
}

impl AccountRecords {
    fn link(&self, account: &HubAccount) -> Result<Arc<Link>, Failure> {
        let node = self.node.upgrade().ok_or_else(|| Failure::new("the node has gone"))?;
        node.link_of(&account.hub).ok_or_else(|| Failure::not_run("the account's network is not connected"))
    }
}

impl Records for AccountRecords {
    fn available(&self) -> bool {
        true
    }

    fn read<'a>(&'a self, account: &'a HubAccount, kind: &'a str, key: &'a str) -> Reply<'a, Option<Record>> {
        Box::pin(async move {
            let link = self.link(account)?;
            match link
                .account_records(&account.account, Request::Read { kind: kind.into(), key: key.into() })
                .await
                .map_err(Failure::new)?
            {
                Response::Read { record: Some(record) } => Ok(Some(Record {
                    version: record.metadata.version,
                    body: if record.metadata.deleted { None } else { Some(record.body) },
                })),
                Response::Read { record: None } => Ok(None),
                _ => Err(Failure::new("the hub answered another account-records reply")),
            }
        })
    }

    fn list<'a>(&'a self, account: &'a HubAccount, kind: &'a str) -> Reply<'a, Vec<Changed>> {
        Box::pin(async move {
            let link = self.link(account)?;
            match link
                .account_records(&account.account, Request::List { kind: kind.into() })
                .await
                .map_err(Failure::new)?
            {
                Response::Listed { entries } => {
                    Ok(entries.into_iter().map(|m| Changed { key: m.key, version: m.version }).collect())
                }
                _ => Err(Failure::new("the hub answered another account-records reply")),
            }
        })
    }

    fn exchange<'a>(
        &'a self,
        account: &'a HubAccount,
        kind: &'a str,
        key: &'a str,
        expected: Option<u64>,
        body: Option<Value>,
    ) -> Reply<'a, Exchanged> {
        Box::pin(async move {
            let link = self.link(account)?;
            let request = match body {
                Some(body) => Request::Write { kind: kind.into(), key: key.into(), expected, body },
                None => Request::Delete { kind: kind.into(), key: key.into(), expected },
            };
            match link.account_records(&account.account, request).await.map_err(Failure::new)? {
                Response::Written { metadata } => Ok(Exchanged::Written { version: metadata.version }),
                Response::Conflict { current } => Ok(Exchanged::Conflict { current: current.map(|m| m.version) }),
                _ => Err(Failure::new("the hub answered another account-records reply")),
            }
        })
    }

    fn watch<'a>(&'a self, account: &'a HubAccount, kind: &'a str) -> Reply<'a, Box<dyn Changes>> {
        Box::pin(async move {
            let link = self.link(account)?;
            let watch = link.watch_account_records(account.account.clone(), kind.into()).map_err(Failure::new)?;
            Ok(Box::new(RecordChanges { watch }) as Box<dyn Changes>)
        })
    }
}

/// The account-records watch, as the goal runtime's change stream: a reconnect
/// or a lag begins again with the current keys.
struct RecordChanges {
    watch: RecordWatch,
}

impl Changes for RecordChanges {
    fn next(&mut self) -> Reply<'_, Vec<Changed>> {
        Box::pin(async move {
            match self.watch.next().await.map_err(Failure::new)? {
                Watched::Current { entries } => {
                    Ok(entries.into_iter().map(|m| Changed { key: m.key, version: m.version }).collect())
                }
                Watched::Changed { metadata } => Ok(vec![Changed { key: metadata.key, version: metadata.version }]),
            }
        })
    }
}

/// The conversation's named context, through the records line's context blocks.
struct NamedContext {
    node: Weak<Node>,
}

impl Context for NamedContext {
    fn available(&self) -> bool {
        true
    }

    fn set<'a>(&'a self, resident: &'a ResidentId, name: &'a str, text: &'a str) -> Reply<'a, ()> {
        Box::pin(async move {
            let node = self.node.upgrade().ok_or_else(|| Failure::new("the node has gone"))?;
            let change = crate::context::Change { by: None, version: "", fact: GOAL_CONTEXT_CHANGED, full: true };
            // An archived conversation still has its goal block taken down.
            let result = if text.is_empty() {
                crate::context::clear(&node, resident, name, change)
            } else {
                crate::context::set(&node, resident, name, text, change)
            };
            result.map_err(|error| Failure::new(error.to_string()))
        })
    }
}

/// The goal services, with their records and context connected to this node.
pub(crate) fn connected(node: Weak<Node>) -> super::Services {
    super::Services {
        records: Arc::new(AccountRecords { node: node.clone() }),
        context: Arc::new(NamedContext { node: node.clone() }),
        reviewer: Arc::new(crate::sentinels::goal_seams::GoalSentinel(node)),
        ..Default::default()
    }
}

