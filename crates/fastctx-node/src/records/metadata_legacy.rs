//! Only explicitly node-owned metadata moves into records. Host control tickets
//! and replies keep their file protocol until their readers change with it.
use std::{io, path::Path};
fn json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> io::Result<()> {
    serde_json::from_slice::<T>(bytes).map(|_| ()).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
pub(super) fn validate(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().and_then(Path::file_name).and_then(|name| name.to_str()).unwrap_or("");
    match parent {
        "turn-stops" => crate::turn_evidence::stopping::validate_legacy(path, bytes),
        "turn-failures" => {
            let value: serde_json::Value = serde_json::from_slice(bytes)?;
            let turn = value
                .get("turn")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "turn diagnostic has no turn"))?;
            if path.file_stem().and_then(|name| name.to_str())
                != Some(fastctx_hands::files::content_hash(turn.as_bytes()).as_str())
            {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "diagnostic belongs to another turn"));
            }
            Ok(())
        }
        "goal-inputs" => crate::goals::colleagues::validate_legacy_receipt(path, bytes),
        "goal-compactions" => json::<crate::goals::engine::compaction::Attempt>(bytes),
        "books" if path.file_name().is_some_and(|name| name == "goal-retry.json") => {
            json::<crate::goals::engine::checkpoint::Checkpoint>(bytes)
        }
        _ => Ok(()),
    }
}
