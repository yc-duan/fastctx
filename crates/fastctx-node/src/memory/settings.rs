//! The account's optional project map; standalone computers keep the same switch locally.

use crate::{Node, town::caller::Caller};
use fastctx_types::{AccountName, HubName, ipc::Failure, memory::Settings};
use std::sync::Arc;

pub(crate) async fn call(
    node: &Arc<Node>,
    hub: Option<HubName>,
    account: Option<AccountName>,
    enabled: Option<bool>,
) -> Result<Settings, Failure> {
    if node.links().is_empty() && hub.is_none() && account.is_none() {
        let _writing = node.writing.lock().await;
        let path = node.world.path().join("memory.json");
        let mut settings: Settings = fastctx_net::link::state::read_or(&path, Settings::default())
            .map_err(|error| Failure::new(error.to_string()))?;
        if let Some(enabled) = enabled {
            settings.enabled = enabled;
            fastctx_net::link::state::write(&path, &settings).map_err(|error| Failure::new(error.to_string()))?;
            node.memory_changed();
        }
        return Ok(settings);
    }
    let (link, account) = crate::profile::scope(node, &Caller::User, hub.as_ref(), account.as_ref())?;
    let settings = link.memory_settings(&account, enabled).await.map_err(Failure::new)?;
    node.memory_changed();
    Ok(settings)
}
