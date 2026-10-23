//! Settings retain hub account identity and never read secret values.
use crate::Node;
use fastctx_net::link::Link;
use fastctx_sentinel::records;
use fastctx_types::{
    AccountName, MachineName,
    access::{HubAccount, Purpose},
    ipc::Failure,
    remote::{RemoteCall, RemoteReply},
    sentinel_settings::*,
};
use std::sync::Arc;

pub(crate) async fn settings(
    node: &Arc<Node>,
    link: &Arc<Link>,
    account: &AccountName,
    change: Option<WriteSettings>,
) -> Result<SettingsReply, Failure> {
    let scope = HubAccount { hub: link.membership().id.clone(), account: account.clone() };
    let records = node.sentinels.records()?;
    let (version, settings) = if let Some(change) = change {
        if let Some(home) = &change.settings.home {
            let view = crate::authorization::current_view(link)?;
            if !view.member(home).is_some_and(|member| member.is_machine())
                || !view.call_rights(&view.me, account, home).hands
            {
                return Err(Failure::refused("the sentinel home is not a machine this account may use"));
            }
        }
        match fastctx_sentinel::settings::write(records, &scope, change).await {
            Ok(record) => {
                (Some(record.version), serde_json::from_value(record.value).map_err(|e| Failure::new(e.to_string()))?)
            }
            Err(records::Error::Conflict { current_version }) => {
                return Ok(SettingsReply::Conflict { current_version });
            }
            Err(error) => return Err(Failure::new(error.to_string())),
        }
    } else {
        fastctx_sentinel::settings::read(records, &scope).await.map_err(|e| Failure::new(e.to_string()))?
    };
    let status = home_status(link, account, settings.home.as_ref()).await;
    let (keys, key_error) = if status == HomeStatus::Ready {
        match node.sentinels.credentials.get() {
            Some(provider) => match tokio::time::timeout(
                std::time::Duration::from_secs(30),
                provider.status(&scope, settings.home.as_ref().expect("ready has a home")),
            )
            .await
            {
                Ok(Ok(keys)) => (Some(keys), None),
                Ok(Err(error)) => (None, Some(error)),
                Err(_) => (None, Some("reading sentinel key status timed out".into())),
            },
            None => (None, Some("sentinel credential service is not connected".into())),
        }
    } else {
        (None, None)
    };
    Ok(SettingsReply::View { view: SettingsView { version, settings, home_status: status, keys, key_error } })
}

pub(crate) async fn home_status(link: &Arc<Link>, account: &AccountName, home: Option<&MachineName>) -> HomeStatus {
    let Some(home) = home else { return HomeStatus::Unset };
    let view = match crate::authorization::current_view(link) {
        Ok(view) => view,
        Err(error) => return HomeStatus::Unavailable { reason: error.message },
    };
    let Some(member) = view.member(home).filter(|member| member.is_machine()) else { return HomeStatus::Forbidden };
    if !view.call_rights(&view.me, account, home).hands {
        return HomeStatus::Forbidden;
    }
    if view.is_me(home) {
        return HomeStatus::Ready;
    }
    if !member.online {
        return HomeStatus::Offline;
    }
    match link.call(home, account, Purpose::Look, RemoteCall::Facts).await {
        Ok(Ok(RemoteReply::Facts(facts))) if facts.sentinels => HomeStatus::Ready,
        Ok(Ok(RemoteReply::Facts(_))) => HomeStatus::TooOld,
        Ok(Ok(_)) => HomeStatus::Unavailable { reason: "the sentinel home did not answer its capabilities".into() },
        Ok(Err(error)) => HomeStatus::Unavailable { reason: error.message },
        Err(error) => HomeStatus::Unavailable { reason: error.to_string() },
    }
}
