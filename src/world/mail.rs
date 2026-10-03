//! Durable inbox publication and permanent deduplication independent of consumer deletion.
use super::{WorldPaths, envelope::Envelope, messages, write_atomic};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Letter {
    pub(crate) id: String,
    /// Opaque resident-layer JSON; the network does not interpret it.
    pub(crate) body: serde_json::Value,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct InboxItem {
    pub(crate) receipt: String,
    pub(crate) from: String,
    pub(crate) letter: Letter,
    pub(crate) path: PathBuf,
}
#[derive(Deserialize, Serialize)]
struct Receipt {
    hash: String,
    consumed: bool,
}

pub(crate) fn accept(
    paths: &WorldPaths,
    env: &Envelope,
    author: &[u8; 32],
) -> Result<bool, String> {
    let opened = env.open()?;
    let letter: Letter = messages::decode(&opened.body, "mail")?;
    let id = hex::encode(super::crypto::sha256(&messages::encode(&(
        author, &letter.id,
    ))?));
    let bytes = messages::encode(env)?;
    let hash = hex::encode(super::crypto::sha256(&bytes));
    let receipts = paths.dir.join("mail-receipts");
    let inbox = paths.dir.join("inbox");
    std::fs::create_dir_all(&receipts)
        .and_then(|()| std::fs::create_dir_all(&inbox))
        .map_err(|e| e.to_string())?;
    let receipt = receipts.join(format!("{id}.json"));
    if let Some(bytes) = super::read_optional(&receipt)? {
        let previous: Receipt = messages::decode(&bytes, "mail receipt")?;
        if previous.hash != hash {
            return Err("mail id reused with different bytes".into());
        }
        return Ok(false);
    }
    let path = inbox.join(format!("{id}.json"));
    if let Some(previous) = super::read_optional(&path)? {
        if previous != bytes {
            return Err("inbox publication conflict".into());
        }
    } else {
        write_atomic(&path, &bytes)?;
    }
    // Only receipt-backed files are exposed by list(). A crash before this commit leaves
    // an unpublished file that the same reliable delivery completes on reconnect.
    write_atomic(
        &receipt,
        &messages::encode(&Receipt {
            hash,
            consumed: false,
        })?,
    )?;
    Ok(true)
}
pub(crate) fn list(paths: &WorldPaths) -> Result<Vec<InboxItem>, String> {
    let inbox = paths.dir.join("inbox");
    if !inbox.exists() {
        return Ok(Vec::new());
    }
    let mut items = Vec::new();
    for entry in std::fs::read_dir(&inbox).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("Invalid inbox name")?
            .to_string();
        let Some(bytes) =
            super::read_optional(&paths.dir.join("mail-receipts").join(format!("{id}.json")))?
        else {
            continue;
        };
        let receipt: Receipt = messages::decode(&bytes, "mail receipt")?;
        if receipt.consumed {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        if hex::encode(super::crypto::sha256(&bytes)) != receipt.hash {
            return Err("inbox integrity failure".into());
        }
        let env: Envelope = messages::decode(&bytes, "inbox envelope")?;
        let opened = env.open()?;
        items.push(InboxItem {
            receipt: id,
            from: opened.header.from,
            letter: messages::decode(&opened.body, "mail")?,
            path,
        });
    }
    items.sort_by(|a, b| a.receipt.cmp(&b.receipt));
    Ok(items)
}
pub(crate) fn acknowledge(paths: &WorldPaths, id: &str) -> Result<(), String> {
    if id.len() != 64 || hex::decode(id).is_err() {
        return Err("Invalid mail receipt".into());
    }
    let receipt_path = paths.dir.join("mail-receipts").join(format!("{id}.json"));
    let bytes = std::fs::read(&receipt_path).map_err(|e| e.to_string())?;
    let mut receipt: Receipt = messages::decode(&bytes, "mail receipt")?;
    receipt.consumed = true;
    write_atomic(&receipt_path, &messages::encode(&receipt)?)?;
    match std::fs::remove_file(paths.dir.join("inbox").join(format!("{id}.json"))) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
