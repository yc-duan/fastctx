//! Account work records public settings and identities; keys are read only by the home.
use crate::{
    Node,
    submitted::{self, Family},
};
use fastctx_types::{
    access::HubAccount,
    ipc::Failure,
    sentinel_settings::{KeyName, KeyStatus, SettingsReply, TryReport, WriteSettings},
};
use std::{sync::Arc, time::Duration};

const SETTINGS: Family = Family {
    name: "call/sentinel-settings",
    at_once: 8,
    within: Duration::from_secs(5 * 60),
    stays: Duration::from_secs(240),
    // A repeated CAS may conflict after the first write already succeeded.
    again: false,
};
const KEY_DELETE: Family = Family {
    name: "call/sentinel-key-delete",
    at_once: 8,
    within: Duration::from_secs(5 * 60),
    stays: Duration::from_secs(240),
    // A new key may have been set after an interrupted deletion.
    again: false,
};
const TRY: Family = Family {
    name: "call/sentinel-try",
    at_once: 8,
    within: Duration::from_secs(5 * 60),
    // The call has a 180-second deadline, including reading its settings.
    stays: Duration::from_secs(150),
    // Retrying can make another paid provider request.
    again: false,
};

#[derive(serde::Serialize, serde::Deserialize)]
struct Settings {
    scope: HubAccount,
    change: WriteSettings,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct DeleteKey {
    scope: HubAccount,
    key: KeyName,
    routed: bool,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Try {
    scope: HubAccount,
    routed: bool,
}

pub(super) fn register(node: &Arc<Node>) {
    submitted::register(node, SETTINGS, |node: Arc<Node>, asked: Settings| async move {
        let link = node
            .link_of(&asked.scope.hub)
            .ok_or_else(|| Failure::not_run("the sentinel account's network is not connected"))?;
        super::super::settings::work(&node, &link, &asked.scope.account, Some(asked.change)).await
    });
    submitted::register(node, KEY_DELETE, |node: Arc<Node>, asked: DeleteKey| async move {
        if asked.routed {
            let link = node
                .link_of(&asked.scope.hub)
                .ok_or_else(|| Failure::not_run("the sentinel account's network is not connected"))?;
            super::super::keys::set_work(&node, &link, &asked.scope.account, asked.key, None).await
        } else {
            super::super::keys::keep_work(&node, &asked.scope, asked.key, None).await
        }
    });
    submitted::register(node, TRY, |node: Arc<Node>, asked: Try| async move {
        if asked.routed {
            let link = node
                .link_of(&asked.scope.hub)
                .ok_or_else(|| Failure::not_run("the sentinel account's network is not connected"))?;
            super::super::trial::route_work(&node, &link, &asked.scope.account).await
        } else {
            super::super::trial::work(&node, &asked.scope).await
        }
    });
}

pub(crate) async fn settings(
    node: &Arc<Node>,
    scope: HubAccount,
    change: WriteSettings,
) -> Result<SettingsReply, Failure> {
    let unit = super::scoped_unit("account", &scope);
    submitted::submit(node, &SETTINGS, &unit, &Settings { scope, change }).await
}
pub(crate) async fn delete_key(
    node: &Arc<Node>,
    scope: &HubAccount,
    key: KeyName,
    routed: bool,
) -> Result<KeyStatus, Failure> {
    let unit = super::scoped_unit("account", scope);
    submitted::submit(node, &KEY_DELETE, &unit, &DeleteKey { scope: scope.clone(), key, routed }).await
}
pub(crate) async fn tried(node: &Arc<Node>, scope: &HubAccount, routed: bool) -> Result<TryReport, Failure> {
    let unit = super::scoped_unit("account", scope);
    submitted::submit(node, &TRY, &unit, &Try { scope: scope.clone(), routed }).await
}
