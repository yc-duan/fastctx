//! Account changes keep their original selectors apart from their resolved target.

use super::{
    Asked,
    scope::{self, Target},
};
use crate::{
    Node,
    submitted::Family,
    town::caller::{self, Caller},
};
use fastctx_types::{Call, Reply, ipc::Failure};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

const fn family(name: &'static str) -> Family {
    // CAS conflicts, audit rows and follow-up effects do not prove a whole try replayable.
    Family { name, at_once: 8, within: Duration::from_secs(30 * 60), stays: Duration::from_secs(240), again: false }
}
const RECORDS: Family = family("call/account-records");
const COLLAB: Family = family("call/collaboration-settings");
const SUBAGENTS: Family = family("call/subagents");
const PROFILE: Family = family("call/profile");
const HISTORY_SWITCH: Family = family("call/history-switch");
const MCP_REMOVE: Family = family("call/mcp-remove");

#[derive(Clone, Serialize, Deserialize)]
enum Resolved {
    Account(Target),
    Own,
}

#[derive(Serialize, Deserialize)]
struct Answer {
    target: Resolved,
    result: Result<Box<Reply>, Failure>,
}

pub(super) fn handles(call: &Call) -> bool {
    match call {
        Call::AccountRecords { request, .. } => matches!(
            request,
            fastctx_types::account_records::Request::Write { .. }
                | fastctx_types::account_records::Request::Delete { .. }
        ),
        Call::CollabWrite { .. } => true,
        Call::HistorySwitchWrite { account: Some(_), .. } => true,
        Call::ProfileSubagents { operation, .. } => operation.version().is_some(),
        // URL servers carry headers and may carry URL credentials. Command
        // entries here are references to a machine, never its private command.
        Call::ProfileWrite { profile, .. } => {
            profile.servers.values().all(|entry| matches!(entry, fastctx_types::profile::Entry::Command { .. }))
        }
        Call::SkillFiles { request, .. } => request.writes(),
        Call::Mcp { mcp: fastctx_types::mcp::McpCall::Remove { .. }, .. } => true,
        _ => false,
    }
}

pub(super) fn register(node: &Arc<Node>) {
    for family in [RECORDS, COLLAB, SUBAGENTS, PROFILE, HISTORY_SWITCH, MCP_REMOVE] {
        crate::submitted::register_resolved(
            node,
            family,
            |node: Arc<Node>, asked: Asked, resolved: Resolved| async move { work(&node, asked, resolved).await },
        );
    }
}

fn user(node: &Node, peer: &fastctx_types::ipc::Peer) -> Result<Caller, Failure> {
    crate::server::users_voice(node, peer)?;
    caller::of(node, peer)
}

fn check(node: &Arc<Node>, asked: &Asked, target: Option<&Resolved>) -> Result<Resolved, Failure> {
    // Credential-bearing calls must never enter a durable recording (2026-11-10).
    if !handles(&asked.call) {
        return Err(Failure::not_run("this is not a submitted account change"));
    }
    let caller = caller::of(node, &asked.peer)?;
    crate::admission::local::admit(node, &asked.peer, &asked.call)?;
    let (hub, account) = match &*asked.call {
        Call::AccountRecords { hub, account, request } => {
            crate::plan::records_allowed(node, &caller, request)?;
            request.validate().map_err(Failure::not_run)?;
            (hub, account)
        }
        Call::SkillFiles { hub, account, request } => {
            user(node, &asked.peer)?;
            request.validate().map_err(Failure::not_run)?;
            (hub, account)
        }
        Call::CollabWrite { hub, account, collab } => {
            user(node, &asked.peer)?;
            collab.validate().map_err(Failure::not_run)?;
            if account.is_none() {
                crate::server::own_settings(hub.as_ref())?;
                if node.connected_anywhere() {
                    return Err(Failure::not_run(
                        "this computer is in a network, where model collaboration is set for each account; name the account",
                    ));
                }
                let logins = collab.callers.iter().chain(collab.callees.iter().map(|callee| &callee.login));
                if let Some(other) = logins.map(|login| &login.machine).find(|machine| !node.is_me(machine)) {
                    return Err(Failure::not_run(format!(
                        "{other} is not this computer, {}: a computer in no network calls and is called only by its own logins",
                        node.machine()
                    )));
                }
                return Ok(Resolved::Own);
            }
            (hub, account)
        }
        Call::HistorySwitchWrite { hub, account, .. } => {
            user(node, &asked.peer)?;
            (hub, account)
        }
        Call::ProfileSubagents { hub, account, operation } => {
            use fastctx_types::profile::{SubagentOperation, Subagents};
            match operation {
                SubagentOperation::Save { agent, .. } => {
                    fastctx_types::profile::validate_subagent(agent).map_err(Failure::not_run)?
                }
                SubagentOperation::Instructions { text, .. } => {
                    Subagents { agents: Vec::new(), instructions: text.clone() }.validate().map_err(Failure::not_run)?
                }
                _ => {}
            }
            (hub, account)
        }
        Call::ProfileWrite { hub, account, profile } => {
            profile.validate().map_err(Failure::not_run)?;
            (hub, account)
        }
        Call::Mcp { hub, account, .. } => (hub, account),
        _ => return Err(Failure::not_run("this is not a submitted account change")),
    };
    let resolved = match target {
        Some(Resolved::Account(target)) => {
            scope::current(node, &caller, target)?;
            Resolved::Account(target.clone())
        }
        Some(Resolved::Own) => return Err(Failure::refused("the account change has no account target")),
        None if matches!(&*asked.call, Call::CollabWrite { .. } | Call::HistorySwitchWrite { .. }) => {
            let account = account.clone().ok_or_else(|| Failure::not_run("name the account"))?;
            let link = crate::world::hub_link(node, hub.as_ref())?;
            Resolved::Account(scope::capture(&link, account)?)
        }
        None => Resolved::Account(scope::resolve(node, &caller, hub.as_ref(), account.as_ref())?),
    };
    if let Resolved::Account(target) = &resolved {
        let link = scope::current(node, &caller, target)?;
        let view = crate::authorization::current_view(&link)?;
        if matches!(&*asked.call, Call::CollabWrite { .. } | Call::HistorySwitchWrite { .. })
            && !view.logged_in(&view.me, &target.scope.account)
        {
            return Err(Failure::refused("this device is not logged in to the selected account"));
        }
        if matches!(&*asked.call, Call::ProfileSubagents { .. } | Call::ProfileWrite { .. } | Call::Mcp { .. }) {
            link.profile_read(&target.scope.account).map_err(Failure::refused)?;
        }
    }
    Ok(resolved)
}

fn admission(call: &Call) -> Result<(Family, serde_json::Value), Failure> {
    // Each key names the part of the resolved account being changed. Versions
    // and requested contents remain in Asked, rather than serializing unrelated objects.
    let value = match call {
        Call::AccountRecords { request, .. } => {
            let key = match request {
                fastctx_types::account_records::Request::Write { key, .. }
                | fastctx_types::account_records::Request::Delete { key, .. } => key,
                _ => return Err(Failure::not_run("record reads are not submitted")),
            };
            (RECORDS, serde_json::json!(["record", request.kind(), key]))
        }
        Call::SkillFiles { request, .. } => (RECORDS, serde_json::json!(["skill", request.name()])),
        Call::CollabWrite { .. } => (COLLAB, serde_json::json!(["collab"])),
        Call::HistorySwitchWrite { .. } => (HISTORY_SWITCH, serde_json::json!(["history-switch"])),
        Call::ProfileSubagents { .. } => (SUBAGENTS, serde_json::json!(["subagents"])),
        Call::ProfileWrite { .. } => (PROFILE, serde_json::json!(["profile"])),
        Call::Mcp { mcp: fastctx_types::mcp::McpCall::Remove { name }, .. } => {
            (MCP_REMOVE, serde_json::json!(["mcp", name]))
        }
        _ => return Err(Failure::not_run("this is not a submitted account change")),
    };
    Ok(value)
}

pub(super) async fn local(node: &Arc<Node>, peer: &fastctx_types::ipc::Peer, call: Call) -> Result<Reply, Failure> {
    let asked = Asked { peer: peer.clone(), call: Box::new(call) };
    let resolved = check(node, &asked, None)?;
    profile_servers(node, &asked, &resolved)?;
    let (family, key) = admission(&asked.call)?;
    let unit = super::unit(&(&resolved, key))?;
    let Answer { target, result } = crate::submitted::submit_resolved(node, &family, &unit, &asked, &resolved).await?;
    check(node, &asked, Some(&target))?;
    if let Resolved::Account(target) = &target
        && let Call::AccountRecords { request, .. } = &*asked.call
        && request.kind() == fastctx_types::plan::KIND
    {
        // Legacy plan records have a resource-ownership check at the hub in
        // addition to account access. Recheck it for successful and failed receipts.
        let link = scope::current(node, &caller::of(node, peer)?, target)?;
        let key = match request {
            fastctx_types::account_records::Request::Write { key, .. }
            | fastctx_types::account_records::Request::Delete { key, .. } => key,
            _ => return Err(Failure::not_run("record reads are not submitted")),
        };
        link.account_records(
            &target.scope.account,
            fastctx_types::account_records::Request::Read { kind: request.kind().to_string(), key: key.clone() },
        )
        .await
        .map_err(Failure::new)?;
    }
    let mut reply = result?;
    if let Resolved::Account(target) = &target {
        let link = scope::current(node, &caller::of(node, peer)?, target)?;
        let acting = crate::around::acting_of(node, &caller::of(node, peer)?);
        let usable = |machine: &fastctx_types::MachineName| {
            link.view().rights(&target.scope.account, machine).hands
                && node.authorize_machine(&acting, &machine.into()).is_ok()
        };
        let metadata = |metadata: &mut fastctx_types::account_records::Metadata| {
            if metadata.by.as_ref().is_some_and(|machine| !usable(machine)) {
                metadata.by = None;
            }
        };
        match &mut *reply {
            Reply::AccountRecords {
                response: fastctx_types::account_records::Response::Written { metadata: value },
            } => metadata(value),
            Reply::AccountRecords {
                response: fastctx_types::account_records::Response::Conflict { current: Some(value) },
            } => metadata(value),
            Reply::SkillFiles { result } => match &mut **result {
                fastctx_types::skill_files::Reply::Written { metadata: value } => metadata(value),
                fastctx_types::skill_files::Reply::Conflict { current: Some(value) } => metadata(value),
                _ => {}
            },
            _ => {}
        }
    }
    Ok(*reply)
}

fn profile_servers(node: &Node, asked: &Asked, resolved: &Resolved) -> Result<(), Failure> {
    if let Call::ProfileWrite { profile, .. } = &*asked.call
        && let Resolved::Account(target) = resolved
    {
        let caller = caller::of(node, &asked.peer)?;
        let link = scope::current(node, &caller, target)?;
        let current = link.profile_read(&target.scope.account).map_err(Failure::new)?;
        if profile.servers != current.profile.servers {
            return Err(Failure::not_run("change MCP servers through fastctx mcp"));
        }
    }
    Ok(())
}

async fn work(node: &Arc<Node>, asked: Asked, resolved: Resolved) -> Result<Answer, Failure> {
    let target = check(node, &asked, Some(&resolved))?;
    profile_servers(node, &asked, &target)?;
    let caller = caller::of(node, &asked.peer)?;
    let link = match &target {
        Resolved::Account(target) => Some(scope::current(node, &caller, target)?),
        Resolved::Own => None,
    };
    let account = match &target {
        Resolved::Account(target) => Some(target.scope.account.clone()),
        Resolved::Own => None,
    };
    let result = async {
        let reply = match (*asked.call, link, account) {
            (Call::AccountRecords { request, .. }, Some(link), Some(account)) => {
                let response = link.account_records(&account, request).await.map_err(Failure::new)?;
                Reply::AccountRecords { response }
            }
            (Call::SkillFiles { request, .. }, Some(link), Some(account)) => {
                let result = link.skill_files(&account, request).await.map_err(Failure::new)?;
                Reply::SkillFiles { result: Box::new(result) }
            }
            (Call::CollabWrite { collab, .. }, Some(link), Some(account)) => {
                let version = link.collab_write(&account, collab).await.map_err(Failure::new)?;
                Reply::CollabWritten { version }
            }
            (Call::CollabWrite { collab, .. }, None, None) => {
                let node = node.clone();
                let version = fastctx_platform::task::blocking(move || crate::own_collab::write(&node, collab)).await?;
                Reply::CollabWritten { version }
            }
            (Call::HistorySwitchWrite { on, .. }, Some(link), Some(account)) => {
                // The original switch writer performs its import and ledger notifications.
                crate::history::switch::write_on(node, &link, &account, on).await?;
                Reply::Done
            }
            (Call::ProfileWrite { profile, .. }, Some(link), Some(account)) => {
                let version = link.profile_write(&account, profile).await.map_err(Failure::new)?;
                Reply::ProfileWritten { version }
            }
            (Call::ProfileSubagents { operation, .. }, Some(link), Some(account)) => {
                let view = link.profile_subagents(&account, operation).await.map_err(Failure::new)?;
                Reply::ProfileSubagents(Box::new(view))
            }
            (Call::Mcp { mcp, .. }, Some(link), Some(account)) => {
                let result = crate::mcp::call(node, link, account, mcp, matches!(caller, Caller::Resident(_))).await?;
                Reply::Mcp { result }
            }
            _ => return Err(Failure::not_run("the recorded account change does not match its target")),
        };
        Ok(Box::new(reply))
    }
    .await;
    // Errors can quote private record content too. Keep them inside the same
    // authorization envelope as a success, under the operation's original target.
    Ok(Answer { target, result })
}
