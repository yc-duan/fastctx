//! Settings are accepted by the target node, not by a hub trusting a forwarded
//! payload. Receiving a replica never applies login changes or host-file consent.
use crate::{Error, json, source::denied};
use fastctx_sync::{
    cursor::Revision,
    identity::{DeviceId, ObjectKey, OperationId, Producer, SourceId},
};
use fastctx_types::{
    AgentName,
    logins::models::{self, ModelChange, ModelEdits},
    stats::MEMORY_ROOMS,
    sync::scope::{ScopeDescriptor, ScopeSubject},
    update,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub device: DeviceId,
    pub value: Setting,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Setting {
    Login {
        login: SourceId,
        agent: AgentName,
        value: LoginSetting,
    },
    MemoryReserve {
        bytes: u64,
    },
    Updates {
        auto: bool,
        chosen: bool,
        relays: bool,
    },
    LoginDiscovery {
        added: Vec<Home>,
        folders: Vec<String>,
    },
    /// Historical preference only. It never authorizes deleting an asset by age.
    LegacyRetention {
        command_artifact_days: u32,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Home {
    pub agent: AgentName,
    pub path: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LoginSetting {
    AutoTitle { on: bool },
    ResetInPlace { on: bool },
    Workbench { on: bool },
    Models { edits: ModelEdits },
    BypassConsent { accept: bool },
}
impl Settings {
    pub fn validate(&self) -> Result<(), Error> {
        match &self.value {
            Setting::Login { agent, value, .. } => {
                let supported = match value {
                    LoginSetting::AutoTitle { .. } => agent.as_str() == "codex",
                    LoginSetting::Models { .. } | LoginSetting::BypassConsent { .. } => agent.as_str() == "claude",
                    LoginSetting::ResetInPlace { .. } | LoginSetting::Workbench { .. } => {
                        ["codex", "claude"].contains(&agent.as_str())
                    }
                };
                if !supported {
                    return Err(json::invalid("this login does not support that setting"));
                }
                if let LoginSetting::Models { edits } = value {
                    if edits.added.len() > models::MOST_ADDED {
                        return Err(json::invalid("too many added login models"));
                    }
                    for id in edits.added.iter().chain(&edits.removed) {
                        models::check(id).map_err(Error::Invalid)?;
                    }
                }
            }
            Setting::MemoryReserve { bytes } if !MEMORY_ROOMS.contains(bytes) => {
                return Err(json::invalid("unsupported memory reserve"));
            }
            Setting::LegacyRetention { command_artifact_days }
                if ![1, 3, 7, 14, 30].contains(command_artifact_days) =>
            {
                return Err(json::invalid("invalid legacy retention choice"));
            }
            _ => {}
        }
        Ok(())
    }
}

/// Uses the original pure model operation. `offered` is evaluated using the
/// target login's host model table and observed aliases before publication.
pub fn change_models(
    before: &ModelEdits,
    change: &ModelChange,
    in_table: impl Fn(&str) -> bool,
    by_family: impl Fn(&str) -> bool,
    offered: impl Fn(&ModelEdits) -> bool,
) -> Result<ModelEdits, Error> {
    let mut after = before.clone();
    after.apply(change, in_table, by_family).map_err(Error::Invalid)?;
    if !offered(&after) {
        return Err(json::invalid("a login offers at least one model; add another before taking this one out"));
    }
    Ok(after)
}
pub fn updates(before: update::Settings, auto: Option<bool>, relays: Option<bool>) -> Setting {
    let after = before.with_preferences(auto, relays);
    Setting::Updates { auto: after.auto, chosen: after.chosen, relays: after.relays }
}

/// A verified result of the target node's original command/admission path. This
/// is supplied from the persistent cause, not deserialized from a caller. Legacy
/// delegation retains the target's old rules instead of imposing an Agent cap.
/// Workbench/consent results require the actual host-file publication outcome;
/// start notifications, preview fingerprints alone and replica arrival do not
/// constitute this evidence. A legacy command may legitimately omit a preview.
pub struct Accepted<'a> {
    pub object: &'a ObjectKey,
    pub acl_revision: Revision,
    pub device: &'a DeviceId,
    pub operation: &'a OperationId,
    pub result: &'a Settings,
}
/// Qualifies a statement of an accepted setting; ordinary changes still need
/// their field authorizer and the login-node Expected arbitration port.
pub fn authorize_source(
    value: &Settings,
    producer: &Producer,
    operation: &OperationId,
    object: &ObjectKey,
    scope: &ScopeDescriptor,
    accepted: Accepted<'_>,
) -> Result<(), Error> {
    value.validate()?;
    if producer != &(Producer::Device { device: accepted.device.clone() })
        || &value.device != accepted.device
        || operation != accepted.operation
        || value != accepted.result
        || object != accepted.object
        || scope.scope != object.scope
        || scope.acl_revision != accepted.acl_revision
        || scope.subject != (ScopeSubject::Machine { device: value.device.clone() })
    {
        return Err(denied("setting lacks the target node's matching accepted command result"));
    }
    Ok(())
}
