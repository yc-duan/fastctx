//! Validate each legacy row before retiring its source. Parsing is bounded
//! by one record even when a source contains years of machine activity.
use fastctx_storage::{Error, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;
fn decode<T: DeserializeOwned>(value: &Value) -> Result<()> {
    serde_json::from_value::<T>(value.clone()).map(|_| ()).map_err(|e| Error::new(e.to_string()))
}
pub(super) fn row(kind: &str, value: &Value) -> Result<()> {
    use fastctx_net::directory as d;
    match kind {
        "accounts" if value.get("level").is_none() => Ok(()),
        "accounts" => decode::<d::Account>(value),
        "members" => decode::<d::Member>(value),
        "shares" => decode::<d::Share>(value),
        "lends" => decode::<d::Assist>(value),
        "plans" => decode::<d::LoginPlan>(value),
        "acting" => decode::<d::ActsFor>(value),
        "projects" | "archived" => decode::<d::Project>(value),
        "tokens" => decode::<super::TokenRecord>(value),
        "removed" => decode::<super::Removed>(value),
        "project_deletions" => decode::<crate::project_deletions::Pending>(value),
        "forgetting" => decode::<crate::forget::Pending>(value),
        "letters" => decode::<super::Letters>(value),
        "due" => decode::<super::DueFor>(value),
        "repairs" => decode::<super::RepairFor>(value),
        "package_targets" => decode::<super::PackageTargets>(value),
        "passwords" => decode::<fastctx_net::sign::Bytes>(value),
        "worked" => decode::<(fastctx_types::MachineName, fastctx_types::Address)>(value),
        "reports" => {
            decode::<jiff::Timestamp>(&value["at"])?;
            decode::<fastctx_types::update::Report>(&value["report"])
        }
        _ => Ok(()),
    }
}
pub(super) fn key(kind: &str, key: &str) -> Result<()> {
    let value = Value::String(key.into());
    match kind {
        "passwords" => decode::<fastctx_types::AccountName>(&value),
        "letters" => decode::<fastctx_types::MachineName>(&value),
        "package_targets" | "reports" => decode::<fastctx_net::sign::PublicKey>(&value),
        _ => Ok(()),
    }
}
pub(super) fn field(name: &str, value: &Value) -> Result<()> {
    match name {
        "hub.id" => decode::<fastctx_net::ids::HubId>(value),
        "hub.name" => decode::<fastctx_types::HubName>(value),
        "hub.public" => decode::<Vec<std::net::SocketAddr>>(value),
        "hub.listen" => decode::<std::net::SocketAddr>(value),
        "hub.self_registration" => decode::<bool>(value),
        "hub.updates" => decode::<fastctx_types::update::Policy>(value),
        "hub.dissolved" => decode::<Option<super::Dissolved>>(value),
        "machine_log.next" | "forgetting.last" => decode::<u64>(value),
        _ => Ok(()),
    }
}
