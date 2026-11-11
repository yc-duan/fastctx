//! Non-secret network changes keep the hub selected by their first asking.

use crate::{Node, submitted::Family};
use fastctx_net::{directory::View, link::Link};
use fastctx_types::{
    AccountName, HubName, MachineName,
    access::{HubId, TownId},
    accounts::AccountChange,
    ipc::Failure,
};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

const FAMILY: Family = Family {
    name: "call/network-changes",
    at_once: 8,
    // A batched update assignment may prepare packages for forty-five minutes.
    within: Duration::from_secs(60 * 60),
    stays: Duration::from_secs(240),
    // An old assignment or rename must not overwrite a newer device's choice.
    again: false,
};

pub(crate) fn takes(change: &AccountChange) -> bool {
    match change {
        // Credential boundaries cover both arguments and results (2026-11-11).
        AccountChange::Token { .. }
        | AccountChange::RevokeToken { .. }
        | AccountChange::SetPassword { .. }
        | AccountChange::OpenAccount { .. }
        | AccountChange::Seen { .. } => false,
        AccountChange::Share { .. }
        | AccountChange::Assist { .. }
        | AccountChange::Plan { .. }
        | AccountChange::StopActing { .. }
        | AccountChange::Direct { .. }
        | AccountChange::Remove { .. }
        | AccountChange::Rename { .. }
        | AccountChange::Group { .. }
        | AccountChange::RenameGroup { .. }
        | AccountChange::Logout
        | AccountChange::SetRole { .. }
        | AccountChange::AcceptRole { .. }
        | AccountChange::SetAccount { .. }
        | AccountChange::Transfer { .. }
        | AccountChange::DeleteAccount { .. }
        | AccountChange::Settings { .. }
        | AccountChange::Updates { .. }
        | AccountChange::Intake { .. }
        | AccountChange::UpdateDue { .. }
        | AccountChange::Dissolve { .. }
        | AccountChange::ShareProject { .. }
        | AccountChange::MoveProject { .. }
        | AccountChange::DeleteProject { .. }
        | AccountChange::Relay { .. } => true,
    }
}

#[derive(Serialize, Deserialize)]
enum Asked {
    Change { hub: Option<HubName>, account: AccountName, change: AccountChange },
    Rename { to: MachineName },
}

#[derive(Serialize, Deserialize)]
enum Resolved {
    Unchanged,
    Here,
    Logout {
        hub: HubId,
        account: AccountName,
    },
    Hub {
        hub: HubId,
        account: AccountName,
        opened: Option<jiff::Timestamp>,
        unit: String,
        observed: Option<String>,
        project: Option<TownId>,
        accounts: Vec<(AccountName, jiff::Timestamp)>,
    },
}

fn capture(link: &Link, account: AccountName, change: &AccountChange) -> Result<Resolved, Failure> {
    let view = crate::authorization::current_view(link)?;
    link.check_account_change(change).map_err(Failure::new)?;
    let opened = view.account(&account).map(|found| found.opened);
    let project = match change {
        AccountChange::ShareProject { town, .. }
        | AccountChange::MoveProject { town, .. }
        | AccountChange::DeleteProject { town } => {
            view.projects.iter().find(|project| project.name == *town).map(|project| project.town.clone())
        }
        _ => None,
    };
    let accounts = match change {
        AccountChange::Share { to, .. } | AccountChange::Assist { to, .. } | AccountChange::MoveProject { to, .. } => {
            vec![to]
        }
        AccountChange::ShareProject { with, .. } => vec![with],
        AccountChange::RenameGroup { owner, .. } => vec![owner],
        _ => Vec::new(),
    }
    .into_iter()
    .filter_map(|name| view.account(name).map(|found| (name.clone(), found.opened)))
    .collect();
    let (unit, known) = unit(&view, &account, change)?;
    let observed = known.then(|| unit.clone());
    Ok(Resolved::Hub { hub: link.membership().id.clone(), account, opened, unit, observed, project, accounts })
}

fn unit(view: &View, account: &AccountName, change: &AccountChange) -> Result<(String, bool), Failure> {
    let machine = match change {
        AccountChange::Share { machine, .. }
        | AccountChange::Assist { machine, .. }
        | AccountChange::Plan { machine, .. }
        | AccountChange::StopActing { machine }
        | AccountChange::Direct { machine, .. }
        | AccountChange::Rename { machine, .. }
        | AccountChange::Group { machine, .. }
        | AccountChange::UpdateDue { machine, .. }
        | AccountChange::Relay { machine, .. } => Some(machine),
        AccountChange::Remove { name, .. } => Some(name),
        _ => None,
    };
    let mut known = true;
    let part = if let Some(machine) = machine {
        match view.member(machine) {
            Some(member) => serde_json::json!(["machine", member.key.to_string()]),
            None => {
                known = false;
                serde_json::json!(["machine-name", machine])
            }
        }
    } else {
        match change {
            AccountChange::RenameGroup { owner, from, .. } => serde_json::json!(["group", owner, from]),
            AccountChange::SetRole { name, .. }
            | AccountChange::SetAccount { name, .. }
            | AccountChange::DeleteAccount { name } => {
                let opened = view.account(name).map(|found| found.opened);
                known = opened.is_some();
                serde_json::json!(["account", name, opened])
            }
            AccountChange::Transfer { to } => {
                let opened = view.account(to).map(|found| found.opened);
                known = opened.is_some();
                serde_json::json!(["account", to, opened])
            }
            AccountChange::Logout | AccountChange::AcceptRole { .. } => serde_json::json!(["account", account]),
            AccountChange::Settings { .. } => serde_json::json!(["registration"]),
            AccountChange::Updates { .. } => serde_json::json!(["updates"]),
            AccountChange::Intake { .. } => serde_json::json!(["intake"]),
            AccountChange::Dissolve { .. } => serde_json::json!(["network"]),
            AccountChange::ShareProject { town, .. }
            | AccountChange::MoveProject { town, .. }
            | AccountChange::DeleteProject { town } => match view.projects.iter().find(|found| found.name == *town) {
                Some(found) => serde_json::json!(["project", found.town]),
                None => {
                    known = false;
                    serde_json::json!(["project-name", town])
                }
            },
            _ => return Err(Failure::not_run("this change has no submitted target")),
        }
    };
    let bytes = serde_json::to_vec(&(&view.hub, part)).map_err(|error| Failure::new(error.to_string()))?;
    Ok((format!("network/{}", fastctx_hands::files::content_hash(&bytes)), known))
}

async fn resolve(node: &Arc<Node>, asked: &Asked) -> Result<Resolved, Failure> {
    match asked {
        Asked::Change { hub, account, change } => {
            if !takes(change) {
                return Err(Failure::not_run("this account change cannot be recorded"));
            }
            let link = crate::world::hub_link(node, hub.as_ref())?;
            if matches!(change, AccountChange::Logout) {
                if link.view().me().is_some_and(|me| me.managed) {
                    crate::managed::stays(node)?;
                }
                return Ok(Resolved::Logout { hub: link.membership().id.clone(), account: account.clone() });
            }
            crate::authorization::awaited_view(&link).await?;
            capture(&link, account.clone(), change)
        }
        Asked::Rename { to } => {
            if node.machine().as_str() == to.as_str() {
                return Ok(Resolved::Unchanged);
            }
            let links = node.links();
            if links.is_empty() {
                return Ok(Resolved::Here);
            }
            crate::reach::settled_each(&links).await;
            let (link, account) = links
                .iter()
                .find_map(|link| link.view().me().and_then(|me| me.home.clone()).map(|account| (link, account)))
                .ok_or_else(|| Failure::new("this computer is a machine of no hub it is on, so no hub renames it"))?;
            crate::authorization::awaited_view(link).await?;
            capture(link, account, &AccountChange::Rename { machine: link.name(), to: to.clone() })
        }
    }
}

pub(crate) fn start(node: &Arc<Node>) {
    crate::submitted::register_resolved(node, FAMILY, |node: Arc<Node>, asked: Asked, resolved: Resolved| async move {
        match resolved {
            Resolved::Unchanged => Ok(()),
            Resolved::Here => {
                let Asked::Rename { to } = asked else { return Err(Failure::not_run("the change has no hub target")) };
                if node.connected_anywhere() {
                    return Err(Failure::not_run("this computer joined a network before the local rename began"));
                }
                crate::world::rename_here(&node, to).await
            }
            Resolved::Logout { hub, account } => {
                // Leaving the last account is allowed while offline. The original
                // writer records LEAVING and repeats its managed-machine check.
                let link = node.link_of(&hub).ok_or_else(|| Failure::not_run("the selected hub is unavailable"))?;
                crate::world::change_on(&node, &link, &account, AccountChange::Logout).await.map(|_| ())
            }
            Resolved::Hub { hub, account, opened, observed, project, accounts, .. } => {
                let link = node.link_of(&hub).ok_or_else(|| Failure::not_run("the selected hub is unavailable"))?;
                let view = crate::authorization::awaited_view(&link)
                    .await
                    .map_err(|failure| Failure::not_run(format!("no change was made: {}", failure.message)))?;
                if let (Some(expected), Some(current)) = (opened, view.account(&account))
                    && current.opened != expected
                {
                    return Err(Failure::not_run("the selected account was replaced"));
                }
                for (name, opened) in accounts {
                    if view.account(&name).is_some_and(|found| found.opened != opened) {
                        return Err(Failure::not_run("a selected account was replaced"));
                    }
                }
                let mut change = match asked {
                    Asked::Change { change, .. } if takes(&change) => change,
                    Asked::Rename { to } => AccountChange::Rename { machine: link.name(), to },
                    _ => return Err(Failure::not_run("this account change cannot be recorded")),
                };
                if let Some(project) = project {
                    match &mut change {
                        AccountChange::ShareProject { town, .. }
                        | AccountChange::MoveProject { town, .. }
                        | AccountChange::DeleteProject { town } => {
                            if let Some(current) = view.projects.iter().find(|found| found.town == project) {
                                *town = current.name.clone();
                            } else if view.projects.iter().any(|found| found.name == *town && found.town != project) {
                                return Err(Failure::not_run("the selected project was replaced"));
                            }
                        }
                        _ => return Err(Failure::not_run("the change has an unexpected project target")),
                    }
                }
                if let Some(expected) = observed {
                    let (current, known) = unit(&view, &account, &change)?;
                    if known && current != expected {
                        return Err(Failure::not_run("the selected object was replaced"));
                    }
                }
                // Owner, super-administrator, lock and login checks remain with
                // the original authoritative hub handler, for every execution.
                let token = crate::world::change_on(&node, &link, &account, change).await?;
                if token.is_some() {
                    // The acknowledged change succeeded. Never retain an unexpected secret.
                    node.log.line("the hub committed a non-secret change and returned an unexpected token; the token was discarded");
                }
                Ok(())
            }
        }
    });
    node.submission_kinds.registered(&FAMILY);
}

async fn submit(node: &Arc<Node>, asked: &Asked) -> Result<(), Failure> {
    // The server admits only the local user. Results contain status alone,
    // including after logout, removal or a successful loss of administrator rights.
    crate::submitted::submit_resolving(node, &FAMILY, asked, || async {
        let resolved = resolve(node, asked).await?;
        let unit = match &resolved {
            Resolved::Hub { unit, .. } => unit.clone(),
            Resolved::Logout { hub, account } => format!("network/{hub}/login/{account}"),
            Resolved::Here | Resolved::Unchanged => "machine-name".into(),
        };
        Ok((unit, resolved))
    })
    .await
}

pub(crate) async fn change(
    node: &Arc<Node>,
    hub: Option<HubName>,
    account: AccountName,
    change: AccountChange,
) -> Result<Option<String>, Failure> {
    if !takes(&change) {
        return Err(Failure::not_run("this account change cannot be recorded"));
    }
    if !node.submission_kinds.has(&FAMILY) {
        return crate::world::change(node, hub.as_ref(), &account, change).await;
    }
    let asked = Asked::Change { hub, account, change };
    match submit(node, &asked).await {
        Err(failure) if crate::submission_fallback::unrecorded(&FAMILY, &failure) => {
            let Asked::Change { hub, account, change } = asked else { unreachable!() };
            if matches!(change, AccountChange::Logout) {
                crate::world::change(node, hub.as_ref(), &account, change).await
            } else {
                Err(failure)
            }
        }
        result => result.map(|()| None),
    }
}

pub(crate) async fn rename(node: &Arc<Node>, to: MachineName) -> Result<(), Failure> {
    if !node.submission_kinds.has(&FAMILY) {
        return crate::world::rename_here(node, to).await;
    }
    submit(node, &Asked::Rename { to }).await
}
