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
    /// Where the letter stands in the order the hub delivered this machine's mail.
    pub(crate) arrival: u64,
}
#[derive(Deserialize, Serialize)]
struct Receipt {
    /// The inbox file's bytes, hashed, so a damaged file is recognized.
    hash: String,
    consumed: bool,
    /// The letter itself (its id and body), hashed. A letter sent again under its id is the
    /// same letter when this matches, whatever envelope it came in.
    #[serde(default)]
    letter: String,
    /// The hub's sequence number of the frame that brought the letter.
    #[serde(default)]
    arrival: u64,
}

/// The identity of a letter's content: the same letter sent twice hashes the same, though each
/// envelope carries its own message counter.
pub(crate) fn letter_hash(letter: &Letter) -> Result<String, String> {
    Ok(hex::encode(super::crypto::sha256(&messages::encode(letter)?)))
}

/// Commits a letter to the inbox; `false` when this author's letter with this id was already
/// accepted. `arrival` is the hub's sequence number of the frame that brought it: letters are
/// opened in that order, which keeps the order each sender sent them in.
pub(crate) fn accept(
    paths: &WorldPaths,
    env: &Envelope,
    author: &[u8; 32],
    arrival: u64,
) -> Result<bool, String> {
    let opened = env.open()?;
    let letter: Letter = messages::decode(&opened.body, "mail")?;
    let id = hex::encode(super::crypto::sha256(&messages::encode(&(
        author, &letter.id,
    ))?));
    let content = letter_hash(&letter)?;
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
        let same = if previous.letter.is_empty() { previous.hash == hash } else { previous.letter == content };
        if !same {
            return Err("mail id reused for a different letter".into());
        }
        return Ok(false);
    }
    // Only receipt-backed files are exposed by list(). A crash before the receipt leaves an
    // unpublished file, which the same letter arriving again replaces.
    write_atomic(&inbox.join(format!("{id}.json")), &bytes)?;
    write_atomic(
        &receipt,
        &messages::encode(&Receipt {
            hash,
            consumed: false,
            letter: content,
            arrival,
        })?,
    )?;
    Ok(true)
}
/// The letters waiting in the inbox, in the order the hub delivered them. A letter whose file
/// or receipt cannot be read is set aside with its reason among the delivery failures, so one
/// damaged file never holds up the letters after it.
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
        let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else {
            set_aside(paths, &path, "unnamed", "the inbox file name is not text")?;
            continue;
        };
        match read_item(paths, &path, &id) {
            Ok(Some(item)) => items.push(item),
            Ok(None) => {}
            Err(reason) => set_aside(paths, &path, &id, &reason)?,
        }
    }
    items.sort_by(|a, b| (a.arrival, &a.receipt).cmp(&(b.arrival, &b.receipt)));
    Ok(items)
}

/// One inbox letter; `None` while it has no receipt yet, or once it was consumed.
fn read_item(paths: &WorldPaths, path: &std::path::Path, id: &str) -> Result<Option<InboxItem>, String> {
    let Some(bytes) =
        super::read_optional(&paths.dir.join("mail-receipts").join(format!("{id}.json")))?
    else {
        return Ok(None);
    };
    let receipt: Receipt = messages::decode(&bytes, "mail receipt")?;
    if receipt.consumed {
        return Ok(None);
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if hex::encode(super::crypto::sha256(&bytes)) != receipt.hash {
        return Err("inbox integrity failure".into());
    }
    let env: Envelope = messages::decode(&bytes, "inbox envelope")?;
    let opened = env.open()?;
    Ok(Some(InboxItem {
        receipt: id.to_string(),
        from: opened.header.from,
        letter: messages::decode(&opened.body, "mail")?,
        path: path.to_path_buf(),
        arrival: receipt.arrival,
    }))
}

/// Moves an unreadable inbox file beside the delivery failures, with a record of why.
fn set_aside(paths: &WorldPaths, path: &std::path::Path, id: &str, reason: &str) -> Result<(), String> {
    let directory = paths.dir.join("delivery-failures");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let kept = directory.join(format!("inbound-{id}.letter"));
    std::fs::rename(path, &kept).map_err(|e| format!("Cannot set aside {}: {e}", path.display()))?;
    write_atomic(
        &directory.join(format!("inbound-{id}.json")),
        &messages::encode(&serde_json::json!({
            "direction": "inbound",
            "reason": reason,
            "letter": kept,
        }))?,
    )
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
