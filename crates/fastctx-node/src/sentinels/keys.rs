//! An account's sentinel keys live only on its sentinel home, in a private
//! file of that machine's World. No account record and no hub keeps them;
//! the hub only carries a key on its way to the home.
use crate::Node;
use fastctx_agent::FutureResult;
use fastctx_net::link::Link;
use fastctx_sentinel::ports::Credentials;
use fastctx_types::{
    AccountName, MachineName,
    access::HubAccount,
    ipc::Failure,
    remote::{RemoteCall, RemoteReply},
    sentinel::Definition,
    sentinel_settings::{KeyName, KeyStatus, Secret, Settings},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::{Arc, Weak},
    time::Duration,
};

#[derive(Default, Serialize, Deserialize)]
struct Keys {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    search: Option<String>,
}
impl Keys {
    fn slot(&mut self, key: KeyName) -> &mut Option<String> {
        match key {
            KeyName::Model => &mut self.model,
            KeyName::Search => &mut self.search,
        }
    }
    fn status(&self) -> KeyStatus {
        KeyStatus { model: self.model.is_some(), search: self.search.is_some() }
    }
}

fn path(node: &Node, scope: &HubAccount) -> PathBuf {
    let name = Sha256::digest(serde_json::to_vec(scope).expect("an account serializes"));
    node.world.path().join("sentinel-keys").join(format!("{name:x}.json"))
}
fn read(node: &Node, scope: &HubAccount) -> Result<Keys, String> {
    match std::fs::read(path(node, scope)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| format!("reading sentinel keys: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Keys::default()),
        Err(error) => Err(format!("reading sentinel keys: {error}")),
    }
}
fn write(node: &Node, scope: &HubAccount, keys: &Keys) -> Result<(), String> {
    let path = path(node, scope);
    if keys.model.is_none() && keys.search.is_none() {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(format!("removing sentinel keys: {error}")),
            _ => Ok(()),
        };
    }
    let directory = path.parent().expect("keys live in a directory");
    fastctx_platform::ensure_private_dir(directory).map_err(|error| format!("keeping sentinel keys: {error}"))?;
    let bytes = serde_json::to_vec(keys).map_err(|error| error.to_string())?;
    fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)
        .map_err(|error| format!("keeping sentinel keys: {error}"))
}

async fn settings(node: &Node, scope: &HubAccount) -> Result<Settings, Failure> {
    let records = node.sentinels.records()?;
    tokio::time::timeout(Duration::from_secs(60), fastctx_sentinel::settings::read(records, scope))
        .await
        .map_err(|_| Failure::not_run("reading the sentinel settings timed out"))?
        .map(|(_, settings)| settings)
        .map_err(|error| Failure::not_run(error.to_string()))
}
fn home_is_here(node: &Node, scope: &HubAccount, settings: &Settings) -> Result<bool, Failure> {
    let link = node.link_of(&scope.hub).ok_or_else(|| Failure::not_run("the account's network is not connected"))?;
    let view = crate::authorization::current_view(&link)?;
    Ok(settings.home.as_ref().is_some_and(|home| view.is_me(home)))
}

/// Keeps or removes a key here, only while this machine is the account's home.
pub(crate) async fn keep_here(
    node: &Arc<Node>,
    scope: &HubAccount,
    key: KeyName,
    value: Option<Secret>,
) -> Result<KeyStatus, Failure> {
    let _held = node.sentinels.keys.lock().await;
    let settings = settings(node, scope).await?;
    if !home_is_here(node, scope, &settings)? {
        return Err(Failure::refused("this machine is not the account's sentinel home"));
    }
    let mut keys = read(node, scope).map_err(Failure::new)?;
    *keys.slot(key) = value.map(|Secret(value)| value).filter(|value| !value.trim().is_empty());
    write(node, scope, &keys).map_err(Failure::new)?;
    Ok(keys.status())
}

/// The user's key, handed from this machine to the account's sentinel home.
pub(crate) async fn set(
    node: &Arc<Node>,
    link: &Arc<Link>,
    account: &AccountName,
    key: KeyName,
    value: Option<Secret>,
) -> Result<KeyStatus, Failure> {
    let scope = HubAccount { hub: link.membership().id.clone(), account: account.clone() };
    let home = settings(node, &scope).await?.home.ok_or_else(|| Failure::refused("choose the sentinel home first"))?;
    let view = crate::authorization::current_view(link)?;
    if view.is_me(&home) {
        return keep_here(node, &scope, key, value).await;
    }
    if !view.call_rights(&view.me, account, &home).hands {
        return Err(Failure::refused("this account may not use the sentinel home"));
    }
    if !view.member(&home).is_some_and(|member| member.online) {
        return Err(Failure::not_run("the sentinel home is offline"));
    }
    let call = RemoteCall::SentinelKey { key, value };
    match link.call(&home, account, call.purpose(), call).await.map_err(|error| Failure::new(error.to_string()))?? {
        RemoteReply::SentinelKeys(status) => Ok(status),
        _ => Err(Failure::new("the sentinel home answered another kind of call")),
    }
}

/// Which keys this machine holds for the account.
pub(crate) fn status_here(node: &Node, scope: &HubAccount) -> Result<KeyStatus, Failure> {
    read(node, scope).map(|keys| keys.status()).map_err(Failure::new)
}

/// Drops the keys of an account whose sentinel home is now another machine.
/// Only a settings read that names another home removes them.
pub(crate) async fn forget_if_moved(node: &Arc<Node>, scope: &HubAccount) {
    if !path(node, scope).exists() {
        return;
    }
    let _held = node.sentinels.keys.lock().await;
    let Ok(settings) = settings(node, scope).await else { return };
    if home_is_here(node, scope, &settings).unwrap_or(true) {
        return;
    }
    match write(node, scope, &Keys::default()) {
        Ok(()) => node.log.line(format_args!("removed the sentinel keys of {}: its home is another machine", scope.account)),
        Err(error) => node.log.line(format_args!("sentinel keys of {}: {error}", scope.account)),
    }
}

/// The node's credential provider: secrets are read only on the home itself.
pub(crate) struct Keystore(pub Weak<Node>);
impl Keystore {
    fn node(&self) -> Result<Arc<Node>, String> {
        self.0.upgrade().ok_or_else(|| "the sentinel node has stopped".into())
    }
    fn key(&self, scope: &HubAccount, definition: &Definition, key: KeyName) -> Result<String, String> {
        let node = self.node()?;
        if !node.is_me(&definition.home) {
            return Err("only the sentinel home holds its keys".into());
        }
        let mut keys = read(&node, scope)?;
        keys.slot(key).take().ok_or_else(|| match key {
            KeyName::Model => "the sentinel model key is not set".into(),
            KeyName::Search => "the sentinel search key is not set".into(),
        })
    }
}
impl Credentials for Keystore {
    fn status<'a>(&'a self, scope: &'a HubAccount, home: &'a MachineName) -> FutureResult<'a, KeyStatus> {
        Box::pin(async move {
            let node = self.node()?;
            let link = node.link_of(&scope.hub).ok_or("the account's network is not connected")?;
            let view = crate::authorization::current_view(&link).map_err(|error| error.message)?;
            if view.is_me(home) {
                return status_here(&node, scope).map_err(|error| error.message);
            }
            let call = RemoteCall::SentinelKeys;
            match link.call(home, &scope.account, call.purpose(), call).await.map_err(|error| error.to_string())? {
                Ok(RemoteReply::SentinelKeys(status)) => Ok(status),
                Ok(_) => Err("the sentinel home answered another kind of call".into()),
                Err(error) => Err(error.message),
            }
        })
    }
    fn model_key<'a>(&'a self, scope: &'a HubAccount, definition: &'a Definition) -> FutureResult<'a, String> {
        Box::pin(async move { self.key(scope, definition, KeyName::Model) })
    }
    fn search_key<'a>(&'a self, scope: &'a HubAccount, definition: &'a Definition) -> FutureResult<'a, String> {
        Box::pin(async move { self.key(scope, definition, KeyName::Search) })
    }
}
