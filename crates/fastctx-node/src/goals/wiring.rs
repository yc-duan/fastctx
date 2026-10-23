//! Connects the goal runtime to the account records and named context the
//! records line provides: a goal is a `goal`-kind account record keyed by its
//! conversation, and its live state rides a "goal" context block.

use std::sync::{Arc, Weak};

use fastctx_net::link::Link;
use fastctx_net::link::account_records::RecordWatch;
use fastctx_types::account_records::{Request, Response, Watched};
use fastctx_types::access::HubAccount;
use fastctx_types::collab::{GoalState, GoalSummary};
use fastctx_types::goals::{Author, Request as GoalRequest, Status, UserChange};
use fastctx_types::ipc::Failure;
use fastctx_types::{Address, NoticeId, ResidentId};
use serde_json::Value;

use super::context::Context;
use super::records::{Changed, Changes, Exchanged, Record, Records, Reply};
use crate::Node;
use crate::colleagues::goals::ColleagueGoals;

type Fut<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

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

/// The colleague-facing side of the goal runtime: a colleague's task is a goal
/// the World opens for it, owned by the conversation that asked it.
pub(crate) fn colleagues(node: Weak<Node>) -> Arc<dyn ColleagueGoals> {
    Arc::new(ColleagueRuntime { node })
}

struct ColleagueRuntime {
    node: Weak<Node>,
}

impl ColleagueRuntime {
    fn here(&self) -> Result<Arc<Node>, Failure> {
        self.node.upgrade().ok_or_else(|| Failure::new("the node has gone"))
    }
}

impl ColleagueGoals for ColleagueRuntime {
    fn ready(&self) -> Result<(), Failure> {
        if self.here()?.goals.available() {
            Ok(())
        } else {
            Err(Failure::not_run(fastctx_types::collab_prompt::GOAL_RUNTIME_REQUIRED))
        }
    }

    fn open<'a>(
        &'a self,
        node: &'a Arc<Node>,
        resident: &'a ResidentId,
        _owner: &'a Address,
        objective: &'a str,
    ) -> Fut<'a, Result<(), Failure>> {
        Box::pin(async move {
            let account = super::scope::resolve(node, resident, None)?;
            let address = Address { machine: node.machine(), resident: resident.clone() };
            // The owner (the asking conversation) is derived from the colleague's
            // own origin inside the operation, so a second open is a no-op.
            let current = super::operations::read(node, &address, &account).await?;
            if current.goal.is_some() {
                return Ok(());
            }
            let request = GoalRequest::Change {
                operation: format!("colleague-open:{resident}"),
                expected: current.version,
                change: UserChange::Open { objective: objective.to_string() },
            };
            super::operations::call(node, address, account, Author::User { machine: node.machine() }, None, request)
                .await
                .map(drop)
        })
    }

    fn reopen_blocked<'a>(
        &'a self,
        node: &'a Arc<Node>,
        resident: &'a ResidentId,
        delivery: &'a NoticeId,
    ) -> Fut<'a, Result<(), Failure>> {
        Box::pin(async move {
            let account = super::scope::resolve(node, resident, None)?;
            let address = Address { machine: node.machine(), resident: resident.clone() };
            let current = super::operations::read(node, &address, &account).await?;
            if !current.goal.as_ref().is_some_and(|goal| goal.status == Status::Blocked) {
                return Ok(());
            }
            // The delivery that unblocked it keys the operation, so the same
            // message cannot reopen the goal twice.
            let request = GoalRequest::Change {
                operation: format!("colleague-reopen:{delivery}"),
                expected: current.version,
                change: UserChange::Resume,
            };
            // The handover delivering that message holds the colleague's action and management gates.
            let author = Author::User { machine: node.machine() };
            super::operations::call_held(node, address, account, author, None, request).await.map(drop)
        })
    }

    fn state<'a>(&'a self, node: &'a Arc<Node>, resident: &'a ResidentId) -> Fut<'a, Result<Option<GoalSummary>, Failure>> {
        Box::pin(async move {
            let account = super::scope::resolve(node, resident, None)?;
            let address = Address { machine: node.machine(), resident: resident.clone() };
            let view = super::operations::read(node, &address, &account).await?;
            Ok(view.goal.map(|goal| {
                let state = match goal.status {
                    Status::Active => GoalState::Active,
                    Status::Complete => GoalState::Complete,
                    Status::Blocked => GoalState::Blocked,
                    Status::Paused => GoalState::Paused,
                };
                let elapsed =
                    (jiff::Timestamp::now().as_millisecond() - goal.created_at.as_millisecond()).max(0) as u64;
                GoalSummary { state, started_at: goal.created_at, elapsed_ms: elapsed }
            }))
        })
    }
}
