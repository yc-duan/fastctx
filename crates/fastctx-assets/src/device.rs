//! Device preferences retain the existing consumer's interpretation. Browser
//! values stay lossless strings; this closed key set excludes drafts and caches.
use crate::{
    Error, json,
    source::{Binding, denied},
};
use fastctx_sync::{identity::DeviceId, store::ingest::SourceTarget};
use fastctx_types::{
    features::Features,
    notification_policy,
    sync::scope::{ScopeDescriptor, ScopeSubject},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub device: DeviceId,
    pub value: Preference,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Preference {
    Features { value: Features },
    Browser { key: String, value: Option<String> },
    AndroidNotifications { json: String },
    AndroidTheme { theme: String, chosen: bool },
}
impl Preferences {
    pub fn validate(&self) -> Result<(), Error> {
        match &self.value {
            Preference::Browser { key, .. } if !browser_key(key) => {
                Err(json::invalid("not a registered device preference key"))
            }
            Preference::AndroidNotifications { json } => notification_policy::validate(json).map_err(Error::Invalid),
            _ => Ok(()),
        }
    }
}
/// Legacy consumers deliberately fall back on unrecognized appearance strings;
/// raw values (including model/effort strings) are not a new execution capability.
pub fn browser_key(key: &str) -> bool {
    const EXACT: &[&str] = &[
        "theme",
        "bodySize",
        "motion",
        "lang",
        "notify",
        "agent",
        "pick",
        "pick:",
        "account",
        "account:",
        "pickerFolds",
        "desk",
        "sidebar",
        "sidebar-width",
        "folded",
        "side-share",
        "side-dock",
        "side-open",
        "side-home-folded",
        "machines.view",
        "machines.folds",
        "machines-map",
        "focusFolds",
        "machine.range",
        "watchers.span",
        "agents.section",
        "agents.view",
        "agents.tab",
        "agents.machine",
        "profile.account",
        "network",
        "settings.tab",
        "talk.settings",
        "me.tab",
        "front-view",
        "activity.view",
        "activity.span",
        "usage.view",
        "workbench.shown",
        "layer",
        "lastSeen",
        "welcomed",
        "path-notice",
        "model:claude",
        "model:codex",
        "effort:claude",
        "effort:codex",
    ];
    EXACT.contains(&key)
        || ["pick:", "account:", "project-view:", "view:", "effort:claude:", "effort:codex:"]
            .iter()
            .any(|prefix| key.strip_prefix(prefix).is_some_and(|suffix| !suffix.is_empty() && !suffix.contains('\0')))
}
/// Invalid legacy language tags remain representable, just as Features::spoken
/// returned no language. A new explicit language choice uses the existing rule.
pub fn validate_language_choice(tag: &str) -> Result<(), Error> {
    fastctx_types::features::unspoken(tag).map_or(Ok(()), |message| Err(Error::Invalid(message)))
}
pub fn authorize_source(
    value: &Preferences,
    target: &SourceTarget,
    binding: &Binding,
    scope: &ScopeDescriptor,
) -> Result<(), Error> {
    value.validate()?;
    binding.check(target, "device.preferences")?;
    binding.device(&value.device)?;
    if scope.scope != binding.object.scope
        || scope.acl_revision != binding.acl_revision
        || scope.subject != (ScopeSubject::Device { device: value.device.clone() })
    {
        return Err(denied("preferences belong to their reporting device"));
    }
    Ok(())
}
