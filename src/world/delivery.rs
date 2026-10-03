//! Permanent delivery failures remain inspectable instead of blocking a reliable queue.
use super::{WorldPaths, envelope::Envelope, messages};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Rejection {
    pub(crate) seq: u64,
    pub(crate) hash: String,
    pub(crate) mail_id: Option<String>,
    pub(crate) reason: String,
}
pub(crate) fn record_outbound(paths: &WorldPaths, rejection: &Rejection) -> Result<(), String> {
    if rejection.hash.len() != 64 || hex::decode(&rejection.hash).is_err() {
        return Err("Invalid delivery rejection hash".into());
    }
    let directory = paths.dir.join("delivery-failures");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    super::write_atomic(
        &directory.join(format!("outbound-{}.json", rejection.hash)),
        &messages::encode(rejection)?,
    )
}
pub(crate) fn record(
    paths: &WorldPaths,
    direction: &str,
    reason: &str,
    env: &Envelope,
) -> Result<(), String> {
    let bytes = messages::encode(env)?;
    let hash = hex::encode(super::crypto::sha256(&bytes));
    let directory = paths.dir.join("delivery-failures");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    super::write_atomic(
        &directory.join(format!("{direction}-{hash}.json")),
        &messages::encode(
            &serde_json::json!({"direction":direction,"reason":reason,"envelope":env}),
        )?,
    )
}
/// Records a letter this machine took from its inbox but could not carry out, with why, beside
/// the letters the network itself could not deliver.
pub(crate) fn record_unopened(
    paths: &WorldPaths,
    item: &super::mail::InboxItem,
    reason: &str,
) -> Result<(), String> {
    let directory = paths.dir.join("delivery-failures");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    super::write_atomic(
        &directory.join(format!("unopened-{}.json", item.receipt)),
        &messages::encode(&serde_json::json!({
            "direction": "inbound",
            "from": item.from,
            "reason": reason,
            "letter": item.letter,
        }))?,
    )
}
pub(crate) fn failures(paths: &WorldPaths) -> Result<Vec<serde_json::Value>, String> {
    let directory = paths.dir.join("delivery-failures");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut failures = Vec::new();
    for item in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
        let path = item.map_err(|e| e.to_string())?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            failures.push(messages::decode(
                &std::fs::read(path).map_err(|e| e.to_string())?,
                "delivery failure",
            )?);
        }
    }
    Ok(failures)
}
