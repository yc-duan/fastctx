//! Sentinel configuration is one versioned account record, without secrets.
use crate::records::{Error, Record, Records};
pub use fastctx_types::sentinel_settings::{RECORD_KEY as KEY, RECORD_KIND as KIND};
use fastctx_types::{
    access::HubAccount,
    sentinel_settings::{Settings, WriteSettings},
};

pub async fn read(records: &dyn Records, scope: &HubAccount) -> Result<(Option<u64>, Settings), Error> {
    match tokio::time::timeout(std::time::Duration::from_secs(60), records.get(scope, KIND, KEY))
        .await
        .map_err(|_| Error::Unavailable("reading sentinel settings timed out".into()))??
    {
        None => Ok((None, Settings::default())),
        Some(record) => {
            let settings = serde_json::from_value(record.value)
                .map_err(|e| Error::Unavailable(format!("invalid sentinel settings: {e}")))?;
            validate(&settings).map_err(Error::Unavailable)?;
            Ok((Some(record.version), settings))
        }
    }
}
pub async fn write(records: &dyn Records, scope: &HubAccount, change: WriteSettings) -> Result<Record, Error> {
    validate(&change.settings).map_err(Error::Unavailable)?;
    let value = serde_json::to_value(change.settings).map_err(|e| Error::Unavailable(e.to_string()))?;
    tokio::time::timeout(
        std::time::Duration::from_secs(60),
        records.compare_exchange(scope, KIND, KEY, change.expected, Some(value)),
    )
    .await
    .map_err(|_| Error::Unavailable("writing sentinel settings timed out; its outcome is unknown".into()))??
    .ok_or_else(|| Error::Unavailable("record service returned no written settings".into()))
}
pub fn validate(settings: &Settings) -> Result<(), String> {
    fastctx_types::sentinel_settings::validate(settings)
}
