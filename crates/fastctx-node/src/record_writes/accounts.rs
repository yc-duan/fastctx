//! Account changes keep their original selectors apart from their resolved target.

use super::{
    Asked,
    scope::{self, Authority, Target},
};
use crate::{
    Node,
    submitted::Family,
    town::caller::{self, Caller},
};
use fastctx_net::link::Link;
use fastctx_types::{AccountName, Call, HubName, Reply, ipc::Failure};
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
// Replaying a committed boolean could overwrite a later choice made on another device.
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

fn user(node: &Node, asked: &Asked) -> Result<(), Failure> {
    crate::server::users_voice(node, &asked.peer)
}

fn validate(node: &Node, asked: &Asked) -> Result<Caller, Failure> {
    // Credential-bearing calls must never enter a durable recording (2026-11-10).
    if !handles(&asked.call) {
        return Err(Failure::not_run("this is not a submitted account change"));
    }
    let caller = caller::of(node, &asked.peer)?;
    crate::admission::local::admit(node, &asked.peer, &asked.call)?;
    match &*asked.call {
        Call::AccountRecords { request, .. } => {
            crate::plan::records_allowed(node, &caller, request)?;
            request.validate().map_err(Failure::new)?;
        }
        Call::SkillFiles { request, .. } => {
            user(node, asked)?;
            request.validate().map_err(Failure::new)?;
        }
        Call::CollabWrite { account, collab, hub } => {
            user(node, asked)?;
            if account.is_none() {
                crate::server::own_settings(hub.as_ref())?;
                collab.validate().map_err(Failure::not_run)?;
            } else {
                collab.validate().map_err(Failure::new)?;
            }
        }
        Call::HistorySwitchWrite { .. } => user(node, asked)?,
        Call::ProfileSubagents { operation, .. } => {
            use fastctx_types::profile::{SubagentOperation, Subagents};
            match operation {
                SubagentOperation::Save { agent, .. } => {
                    fastctx_types::profile::validate_subagent(agent).map_err(Failure::new)?
                }
                SubagentOperation::Instructions { text, .. } => {
                    Subagents { agents: Vec::new(), instructions: text.clone() }.validate().map_err(Failure::new)?
                }
                _ => {}
            }
        }
        Call::ProfileWrite { profile, .. } => {
            // The original writer replaces subagents before validating the profile.
            let mut input = profile.clone();
            input.subagents = None;
            input.validate().map_err(Failure::new)?;
        }
        Call::Mcp { .. } => {}
        _ => return Err(Failure::not_run("this is not a submitted account change")),
    }
    Ok(caller)
}

fn selectors(call: &Call) -> (Option<&HubName>, Option<&AccountName>) {
    match call {
        Call::AccountRecords { hub, account, .. }
        | Call::SkillFiles { hub, account, .. }
        | Call::CollabWrite { hub, account, .. }
        | Call::HistorySwitchWrite { hub, account, .. }
        | Call::ProfileSubagents { hub, account, .. }
        | Call::ProfileWrite { hub, account, .. }
        | Call::Mcp { hub, account, .. } => (hub.as_ref(), account.as_ref()),
        _ => (None, None),
    }
}

fn authority(call: &Call) -> Authority {
    match call {
        Call::CollabWrite { .. } | Call::HistorySwitchWrite { .. } => Authority::LoggedIn,
        _ => Authority::Acting,
    }
}

async fn resolve(node: &Arc<Node>, asked: &Asked) -> Result<Resolved, Failure> {
    let caller = caller::of(node, &asked.peer)?;
    let (hub, account) = selectors(&asked.call);
    let target = if matches!(&*asked.call, Call::CollabWrite { account: None, .. }) {
        Resolved::Own
    } else {
        scope::selection(node, hub).await?;
        let target = if matches!(authority(&asked.call), Authority::LoggedIn) {
            let account = account.cloned().ok_or_else(|| Failure::not_run("name the account"))?;
            let link = crate::world::hub_link(node, hub)?;
            scope::capture(&link, account, Authority::LoggedIn)?
        } else {
            scope::resolve(node, &caller, hub, account)?
        };
        Resolved::Account(target)
    };
    execution(node, asked, &target)?;
    Ok(target)
}

async fn ready(node: &Node, target: &Resolved) -> Result<(), Failure> {
    if let Resolved::Account(target) = target {
        scope::ready(node, target).await?;
    }
    Ok(())
}

fn authorize(node: &Node, asked: &Asked, target: &Resolved) -> Result<Option<Arc<Link>>, Failure> {
    let caller = caller::of(node, &asked.peer)?;
    crate::admission::local::admit(node, &asked.peer, &asked.call)?;
    match target {
        Resolved::Own => {
            user(node, asked)?;
            Ok(None)
        }
        Resolved::Account(target) => {
            if matches!(authority(&asked.call), Authority::LoggedIn) {
                user(node, asked)?;
            }
            let link = scope::current(node, &caller, target, authority(&asked.call))?;
            if matches!(&*asked.call, Call::ProfileSubagents { .. } | Call::ProfileWrite { .. } | Call::Mcp { .. }) {
                link.profile_read(&target.scope.account).map_err(Failure::new)?;
            }
            Ok(Some(link))
        }
    }
}

fn execution(node: &Node, asked: &Asked, target: &Resolved) -> Result<Option<Arc<Link>>, Failure> {
    let link = authorize(node, asked, target)?;
    if let (Resolved::Own, Call::CollabWrite { collab, .. }) = (target, &*asked.call) {
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
    }
    if let (Call::ProfileWrite { profile, .. }, Resolved::Account(target), Some(link)) = (&*asked.call, target, &link) {
        let current = link.profile_read(&target.scope.account).map_err(Failure::new)?;
        if profile.servers != current.profile.servers {
            return Err(Failure::not_run("change MCP servers through fastctx mcp"));
        }
    }
    Ok(link)
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
    validate(node, &asked)?;
    let (family, key) = admission(&asked.call)?;
    let Answer { target, result } = crate::submitted::submit_resolving(node, &family, &asked, || async {
        let resolved = resolve(node, &asked).await?;
        let unit = super::unit(&(&resolved, key))?;
        Ok((unit, resolved))
    })
    .await?;
    // The work is recorded, and may be done: a view that does not come now
    // leaves only whether this caller may read what came of it unjudged.
    ready(node, &target).await.map_err(|_| {
        Failure::new(
            "the change was submitted; what came of it cannot be shown until the hub's current account view is here",
        )
    })?;
    let link = authorize(node, &asked, &target)?;
    if let Resolved::Account(target) = &target
        && let Call::AccountRecords { request, .. } = &*asked.call
        && request.kind() == fastctx_types::plan::KIND
    {
        // Legacy plan records have a resource-ownership check at the hub in
        // addition to account access. Recheck it for successful and failed receipts.
        let link = link.as_ref().ok_or_else(|| Failure::refused("the record has no account target"))?;
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
    if let (Resolved::Account(target), Some(link)) = (&target, link) {
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

async fn work(node: &Arc<Node>, asked: Asked, target: Resolved) -> Result<Answer, Failure> {
    // Restarted jobs can begin before their links have installed a current view.
    ready(node, &target)
        .await
        .map_err(|failure| Failure::not_run(format!("no change was made: {}", failure.message)))?;
    validate(node, &asked)?;
    let link = execution(node, &asked, &target)?;
    let caller = caller::of(node, &asked.peer)?;
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
    // Errors can quote private record content too: keep the original target on both outcomes.
    Ok(Answer { target, result })
}
