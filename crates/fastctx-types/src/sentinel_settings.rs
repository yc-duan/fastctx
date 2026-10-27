//! Account-owned sentinel settings. Secret values never cross this contract.
use crate::{MachineName, agent::ModelSettings};
use serde::{Deserialize, Serialize};

/// The account record that holds these settings.
pub const RECORD_KIND: &str = "sentinel-settings";
pub const RECORD_KEY: &str = "default";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub home: Option<MachineName>,
    pub model: Option<ModelSettings>,
    pub search: Option<SearchSettings>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchSettings {
    pub provider: SearchProvider,
    pub endpoint: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchProvider {
    Tavily,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteSettings {
    /// `None` means the record must not exist yet.
    pub expected: Option<u64>,
    pub settings: Settings,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyStatus {
    pub model: bool,
    pub search: bool,
}
/// Which of the account's sentinel keys a call sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyName {
    Model,
    Search,
}
/// A key on its way to the sentinel home. It serializes as the plain string
/// and never appears in debug output or logs.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(pub String);
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HomeStatus {
    Unset,
    Forbidden,
    Offline,
    Ready,
    TooOld,
    Unavailable { reason: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsView {
    pub version: Option<u64>,
    pub settings: Settings,
    pub home_status: HomeStatus,
    /// Unknown is distinct from missing when the home or its secret store is offline.
    pub keys: Option<KeyStatus>,
    pub key_error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SettingsReply {
    View { view: SettingsView },
    Conflict { current_version: Option<u64> },
}
