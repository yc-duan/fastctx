//! Command definitions stay on their execution machine. A hub profile is
//! the sole roster; its acknowledged version orders writes and collection.

use crate::Node;
use fastctx_types::{
    AccountName,
    access::HubId,
    profile::{CommandMcp, Entry},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
struct Kept {
    account_opened: jiff::Timestamp,
    version: u64,
    server: CommandMcp,
}

fn path(node: &Node, hub: &HubId, account: &AccountName) -> PathBuf {
    node.world.path().join("mcp").join(hub.as_str()).join(format!("{account}.json"))
}

fn read(path: &std::path::Path) -> Result<BTreeMap<String, Kept>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("reading {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(format!("reading {}: {e}", path.display())),
    }
}

fn write(path: &std::path::Path, entries: &BTreeMap<String, Kept>) -> Result<(), String> {
    fastctx_platform::ensure_private_dir(path.parent().expect("MCP directory")).map_err(|e| e.to_string())?;
    fastctx_platform::publish(
        path,
        &serde_json::to_vec(entries).expect("commands serialize"),
        fastctx_platform::Permissions::Private,
    )
    .map_err(|e| format!("writing {}: {e}", path.display()))
}

pub(super) fn get(node: &Node, hub: &HubId, account: &AccountName, name: &str) -> Result<CommandMcp, String> {
    fastctx_types::profile::name_of(name)?;
    let _one = node.mcp_keeping.lock().expect("never poisoned");
    let link = node.link_of(hub).ok_or("the account's hub is no longer joined")?;
    let view = link.view();
    let listed = view
        .account(account)
        .and_then(|account| account.profile.as_ref())
        .and_then(|profile| profile.servers.get(name));
    if !matches!(listed, Some(Entry::Command { machine }) if view.is_me(machine)) {
        return Err(format!("{name} is not registered on this machine"));
    }
    let opened = view.account(account).ok_or("the account was removed")?.opened;
    read(&path(node, hub, account))?
        .remove(name)
        .filter(|kept| kept.account_opened == opened)
        .map(|kept| kept.server)
        .ok_or_else(|| {
            format!("{name} is registered, but its command has not been stored on this machine; add it again")
        })
}

pub(super) async fn keep(
    node: &Node,
    hub: &HubId,
    account: &AccountName,
    name: String,
    version: u64,
    server: CommandMcp,
) -> Result<(), String> {
    fastctx_types::profile::name_of(&name)?;
    super::process::validate(&server)?;
    let link = node.link_of(hub).ok_or("the account's hub is no longer joined")?;
    link.refresh().await?;
    let _one = node.mcp_keeping.lock().expect("never poisoned");
    let view = link.view();
    let profile =
        view.account(account).and_then(|account| account.profile.as_ref()).ok_or("the account has no profile")?;
    if profile.version != version
        || !matches!(profile.servers.get(&name), Some(Entry::Command { machine }) if view.is_me(machine))
    {
        return Err("the profile changed before the command arrived; read it and add the server again".into());
    }
    let path = path(node, hub, account);
    let mut entries = read(&path)?;
    if entries.get(&name).is_some_and(|kept| kept.version > version) {
        return Err("a newer command is already stored".into());
    }
    entries
        .insert(name, Kept { account_opened: view.account(account).expect("checked account").opened, version, server });
    write(&path, &entries)
}

pub(crate) fn sweep(node: &Node, hub: &HubId) -> Result<(), String> {
    let _one = node.mcp_keeping.lock().expect("never poisoned");
    let Some(link) = node.link_of(hub) else { return Ok(()) };
    let view = link.view();
    if !view.profiles || view.me().is_none() {
        return Ok(());
    }
    let dir = node.world.path().join("mcp").join(hub.as_str());
    let paths = match std::fs::read_dir(&dir) {
        Ok(paths) => paths,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    for path in paths {
        let path = path.map_err(|e| e.to_string())?.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Some(account) = path.file_stem().and_then(|v| v.to_str()).and_then(|v| v.parse::<AccountName>().ok())
        else {
            continue;
        };
        let mut entries = read(&path)?;
        let before = entries.len();
        let found = view.account(&account);
        let mut deleted = Vec::new();
        entries.retain(|name, kept| {
            let retain = match found {
                None => false,
                Some(account) if account.opened != kept.account_opened => false,
                Some(account) => account.profile.as_ref().is_none_or(|profile| {
                    profile.version < kept.version
                        || matches!(profile.servers.get(name), Some(Entry::Command { machine }) if view.is_me(machine))
                }),
            };
            if !retain {
                deleted.push(name.clone());
            }
            retain
        });
        if entries.len() != before {
            // Remove logs first, so a failed removal remains discoverable on the next sweep.
            for name in deleted {
                delete_file(&log(node, hub, &account, &name))?;
            }
            if entries.is_empty() {
                delete_file(&path)?;
            } else {
                write(&path, &entries)?;
            }
        }
    }
    Ok(())
}

pub(super) fn log(node: &Node, hub: &HubId, account: &AccountName, name: &str) -> PathBuf {
    node.world.path().join("mcp").join(hub.as_str()).join("logs").join(account.as_str()).join(format!("{name}.log"))
}
fn delete_file(path: &std::path::Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

pub(crate) fn leave(node: &Node, hub: &HubId) -> Result<(), String> {
    let _one = node.mcp_keeping.lock().expect("never poisoned");
    match std::fs::remove_dir_all(node.world.path().join("mcp").join(hub.as_str())) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

pub(super) fn keep_log(
    node: &Node,
    hub: &HubId,
    account: &AccountName,
    name: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let _one = node.mcp_keeping.lock().expect("never poisoned");
    if !read(&path(node, hub, account))?.contains_key(name) {
        return Ok(());
    }
    let path = log(node, hub, account, name);
    fastctx_platform::ensure_private_dir(path.parent().expect("MCP log directory")).map_err(|e| e.to_string())?;
    fastctx_platform::publish(&path, bytes, fastctx_platform::Permissions::Private).map_err(|e| e.to_string())
}

pub(super) fn status(
    node: &Node,
    hub: &HubId,
    account: &AccountName,
    names: &[String],
) -> Result<std::collections::BTreeSet<String>, String> {
    let _one = node.mcp_keeping.lock().expect("never poisoned");
    let kept = read(&path(node, hub, account))?;
    let opened = node.link_of(hub).and_then(|link| link.view().account(account).map(|a| a.opened));
    Ok(names.iter().filter(|name| kept.get(*name).is_some_and(|k| Some(k.account_opened) == opened)).cloned().collect())
}
