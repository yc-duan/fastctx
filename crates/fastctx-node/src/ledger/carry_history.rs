//! Identify replayed history revisions independently of the importing machine's current name.

use fastctx_net::{
    ledger::{Entry, File},
    topic_history,
};

pub(super) fn matches(source: &Entry, expected: &[File], received: &[File]) -> bool {
    // Earlier importers carried history files unchanged, including their old attribution.
    received == expected || (topic_history::has_history(&source.files) && received == source.files)
}

pub(super) fn stamp(source: &Entry, files: &mut [File]) -> Result<(), String> {
    let Some(mut stored) = topic_history::decode(files)? else { return Ok(()) };
    let source = serde_json::to_vec(&("standalone-project-history-v1", source)).map_err(|error| error.to_string())?;
    stored.submission = Some(fastctx_net::sign::sha256(&source).iter().map(|byte| format!("{byte:02x}")).collect());
    let path = topic_history::path(&stored.revision.name, stored.revision.version, "json");
    let metadata = files.iter_mut().find(|file| file.path == path).expect("decoded history metadata");
    metadata.bytes.0 = serde_json::to_vec(&stored).map_err(|error| error.to_string())?;
    Ok(())
}
