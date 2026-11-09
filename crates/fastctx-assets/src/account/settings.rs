//! Closed public settings shapes never include credential values or live status.
use crate::{Error, json};
use fastctx_types::{history_switch::HistorySwitch, sentinel_settings};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "setting", rename_all = "snake_case", deny_unknown_fields)]
pub enum Settings {
    History { value: HistorySwitch },
    Sentinel { value: sentinel_settings::Settings },
}

pub fn decode(body: &[u8]) -> Result<Settings, Error> {
    let object = json::object(body)?;
    let fields = object
        .get("value")
        .and_then(Value::as_object)
        .ok_or_else(|| json::invalid("settings require an object value"))?;
    match object.get("setting").and_then(Value::as_str) {
        Some("history") => json::exact(fields, &["on"])?,
        Some("sentinel") => {
            json::exact(fields, &["home", "model", "search"])?;
            for (key, allowed) in [
                ("model", &["protocol", "endpoint", "model", "output_limit"][..]),
                ("search", &["provider", "endpoint"][..]),
            ] {
                if let Some(value) = fields.get(key).filter(|v| !v.is_null()) {
                    json::exact(value.as_object().ok_or_else(|| json::invalid("invalid settings section"))?, allowed)?;
                }
            }
        }
        _ => return Err(json::invalid("unsupported account settings subtype")),
    }
    let settings: Settings = json::typed(Value::Object(object))?;
    if let Settings::Sentinel { value } = &settings {
        sentinel_settings::validate(value).map_err(Error::Invalid)?;
    }
    Ok(settings)
}

pub fn import(shape: &str, body: &[u8]) -> Result<Vec<u8>, Error> {
    let raw: Value = serde_json::from_slice(body).map_err(|_| json::invalid("invalid legacy settings"))?;
    // Import the old public typed shape. Extra keys ignored by the old reader
    // are not copied as credentials; the migration envelope keeps the original.
    let settings = match shape {
        "history-switch/v1" => Settings::History { value: json::typed(raw)? },
        "sentinel-settings/v1" => Settings::Sentinel { value: json::typed(raw)? },
        _ => return Err(json::invalid("unsupported legacy settings shape")),
    };
    let out = json::bytes(&settings)?;
    decode(&out)?;
    Ok(out)
}

/// Status and key existence come from the authenticated runtime projection.
/// Unknown keys stay None; an unavailable home never becomes a missing key.
pub fn sentinel_view(
    body: &[u8],
    legacy_version: Option<u64>,
    home_status: sentinel_settings::HomeStatus,
    keys: Option<sentinel_settings::KeyStatus>,
    key_error: Option<String>,
) -> Result<sentinel_settings::SettingsView, Error> {
    let Settings::Sentinel { value } = decode(body)? else {
        return Err(json::invalid("not sentinel settings"));
    };
    if !matches!(home_status, sentinel_settings::HomeStatus::Ready) && (keys.is_some() || key_error.is_some()) {
        return Err(json::invalid("non-ready home cannot report a credential query result"));
    }
    if keys.is_some() && key_error.is_some() {
        return Err(json::invalid("credential result cannot be both known and failed"));
    }
    Ok(sentinel_settings::SettingsView { version: legacy_version, settings: value, home_status, keys, key_error })
}

pub fn history_view(body: &[u8], supported: bool) -> Result<HistorySwitch, Error> {
    let Settings::History { value } = decode(body)? else {
        return Err(json::invalid("not history settings"));
    };
    Ok(HistorySwitch { on: supported && value.on })
}
