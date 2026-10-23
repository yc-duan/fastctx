//! Sentinel configuration is one versioned account record, without secrets.
use crate::records::{Error, Record, Records};
use fastctx_types::{
    access::HubAccount,
    sentinel_settings::{Settings, WriteSettings},
};
pub use fastctx_types::sentinel_settings::{RECORD_KEY as KEY, RECORD_KIND as KIND};

pub async fn read(records: &dyn Records, scope: &HubAccount) -> Result<(Option<u64>, Settings), Error> {
    match tokio::time::timeout(std::time::Duration::from_secs(60), records.get(scope, KIND, KEY))
        .await
        .map_err(|_| Error::Unavailable("reading sentinel settings timed out".into()))??
    {
        None => Ok((None, Settings::default())),
        Some(record) => Ok((
            Some(record.version),
            serde_json::from_value(record.value)
                .map_err(|e| Error::Unavailable(format!("invalid sentinel settings: {e}")))?,
        )),
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
    if let Some(model) = &settings.model {
        endpoint(&model.endpoint)?;
        if model.model.trim().is_empty() || model.model.len() > 256 {
            return Err("invalid sentinel model name".into());
        }
    }
    if let Some(search) = &settings.search {
        endpoint(&search.endpoint)?;
    }
    Ok(())
}
fn endpoint(text: &str) -> Result<(), String> {
    if text.len() > 4096 {
        return Err("sentinel endpoint is too long".into());
    }
    let url = url::Url::parse(text).map_err(|e| e.to_string())?;
    let loopback = url.host_str().is_some_and(|h| {
        h == "localhost" || h.trim_matches(['[', ']']).parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
    });
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err("sentinel endpoint requires HTTPS".into());
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err("sentinel endpoint must not contain credentials or a fragment".into());
    }
    Ok(())
}
