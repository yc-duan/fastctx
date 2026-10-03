//! Append-only signed file bundles. Project attributes are ordinary signed JSON files.
use super::{WorldPaths, crypto, identity, members::MemberTable, messages};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct LedgerFile {
    pub(crate) path: String,
    /// Base64 on the wire: an entry travels whole in one control message (`fits`).
    #[serde(with = "base64_bytes")]
    pub(crate) bytes: Vec<u8>,
}

mod base64_bytes {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub(super) fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&super::crypto::b64_encode(bytes))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        super::crypto::b64_decode(&String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// Whether an entry, once signed, fits the one control message the hub takes it in: the signed
/// entry is the base64 body of an envelope whose header, signature and frame stay under 4 KiB.
pub(crate) fn fits(entry: &Entry) -> Result<bool, String> {
    let mut entry = entry.clone();
    entry.id = entry.content_id()?;
    let signed = SignedEntry {
        entry: String::from_utf8(messages::encode(&entry)?).expect("JSON is UTF-8"),
        // An Ed25519 signature is 64 bytes, 88 characters of base64.
        sig: "A".repeat(88),
    };
    let body = messages::encode(&signed)?.len();
    Ok(body.div_ceil(3) * 4 <= super::envelope::MAX_CONTROL_MESSAGE_BYTES - 4 * 1024)
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ProjectUpdate {
    /// File in this bundle whose JSON contains a `machines` array.
    pub(crate) path: String,
    /// Compare-and-swap against the current project version (zero creates the town).
    pub(crate) previous: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Entry {
    pub(crate) scope: String,
    pub(crate) id: String,
    pub(crate) author: String,
    pub(crate) files: Vec<LedgerFile>,
    pub(crate) project: Option<ProjectUpdate>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SignedEntry {
    pub(crate) entry: String,
    pub(crate) sig: String,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Row {
    pub(crate) seq: u64,
    pub(crate) signed: SignedEntry,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Town {
    pub(crate) seq: u64,
    pub(crate) project_version: u64,
    pub(crate) machines: Vec<String>,
}
impl SignedEntry {
    pub(crate) fn sign(entry: &Entry, key: &identity::SigningIdentity) -> Result<Self, String> {
        let mut entry = entry.clone();
        entry.id = entry.content_id()?;
        entry.validate()?;
        let bytes = messages::encode(&entry)?;
        Ok(Self {
            sig: crypto::b64_encode(&key.sign("ledger_entry", &bytes)),
            entry: String::from_utf8(bytes).expect("JSON is UTF-8"),
        })
    }
    pub(crate) fn parse(&self) -> Result<Entry, String> {
        messages::decode(self.entry.as_bytes(), "ledger entry")
    }
    pub(crate) fn verify(&self, members: &MemberTable) -> Result<Entry, String> {
        let entry = self.parse()?;
        let key = members
            .trusted_key(&entry.author)
            .ok_or("untrusted ledger author")?;
        self.verify_key(&key)
    }
    /// Ledger synchronization reads historical statements, not a live invocation. The hub
    /// rejects new submissions by revoked devices; their existing entries remain readable.
    fn verify_history(&self, members: &MemberTable) -> Result<Entry, String> {
        let entry = self.parse()?;
        let signed = members
            .records
            .get(&entry.author)
            .ok_or("Unknown historical ledger author")?;
        if members.blocked.contains(&entry.author) {
            return Err("Conflicting historical admission".into());
        }
        let record = super::members::verify_record(signed)?;
        self.verify_key(&crypto::b64_array(
            &record.node_pub,
            "historical ledger author",
        )?)
    }
    fn verify_key(&self, key: &[u8; 32]) -> Result<Entry, String> {
        let entry = self.parse()?;
        if messages::encode(&entry)? != self.entry.as_bytes() {
            return Err("ledger entry is not in canonical wire form".into());
        }
        identity::verify(
            key,
            "ledger_entry",
            self.entry.as_bytes(),
            &crypto::b64_decode(&self.sig)?,
        )?;
        entry.validate()?;
        Ok(entry)
    }
}
impl Entry {
    /// The content-derived identity makes equivocation under one id detectable even when
    /// two recipients have never exchanged their copies.
    pub(crate) fn content_id(&self) -> Result<String, String> {
        Ok(hex::encode(crypto::sha256(&messages::encode(&(
            &self.scope,
            &self.author,
            &self.files,
            &self.project,
        ))?)))
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        validate_scope(&self.scope)?;
        if self.id.len() != 64 || hex::decode(&self.id).is_err() {
            return Err("ledger id must be a 32-byte hex identifier".into());
        }
        if self.id != self.content_id()? {
            return Err("ledger id does not match its content".into());
        }
        if self.files.is_empty() {
            return Err("ledger entry has no files".into());
        }
        let mut paths = BTreeSet::new();
        for file in &self.files {
            validate_path(&file.path)?;
            if !paths.insert(file.path.to_lowercase()) {
                return Err("duplicate ledger path".into());
            }
        }
        for a in &self.files {
            for b in &self.files {
                if a.path
                    .to_lowercase()
                    .starts_with(&format!("{}/", b.path.to_lowercase()))
                {
                    return Err("ledger bundle has a file/directory collision".into());
                }
            }
        }
        if let Some(update) = &self.project {
            self.machines(update)?;
        }
        Ok(())
    }
    pub(crate) fn machines(&self, update: &ProjectUpdate) -> Result<Vec<String>, String> {
        let file = self
            .files
            .iter()
            .find(|file| file.path == update.path)
            .ok_or("project attributes file is absent")?;
        let value: serde_json::Value = messages::decode(&file.bytes, "project attributes")?;
        let machines: Vec<String> = serde_json::from_value(
            value
                .get("machines")
                .cloned()
                .ok_or("project attributes need machines")?,
        )
        .map_err(|e| e.to_string())?;
        if machines.is_empty() {
            return Err("project machines cannot be empty".into());
        }
        for machine in &machines {
            super::validate_node_name(machine)?;
        }
        Ok(machines)
    }
}
fn validate_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.starts_with('/') {
        return Err("ledger path must be relative".into());
    }
    for part in path.split('/') {
        let base = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || part
                .chars()
                .any(|c| c.is_control() || "\\:<>\"|?*".contains(c))
            || ["CON", "PRN", "AUX", "NUL"].contains(&base.as_str())
            || (base.starts_with("COM") || base.starts_with("LPT"))
                && base.len() == 4
                && base.as_bytes()[3].is_ascii_digit()
        {
            return Err(format!("ledger path has a nonportable component: {path}"));
        }
    }
    Ok(())
}
fn validate_scope(scope: &str) -> Result<(), String> {
    if scope.contains('/') || scope != scope.to_lowercase() {
        return Err("A town scope must be one lowercase directory name".into());
    }
    validate_path(scope)
}
pub(crate) fn directory(paths: &WorldPaths, scope: &str) -> Result<PathBuf, String> {
    validate_scope(scope)?;
    Ok(paths.dir.join("towns").join(scope).join("ledger"))
}
pub(crate) fn cursors(paths: &WorldPaths) -> Result<BTreeMap<String, u64>, String> {
    match super::read_optional(&paths.dir.join("ledger-cursors.json"))? {
        Some(bytes) => messages::decode(&bytes, "ledger cursors"),
        None => Ok(BTreeMap::new()),
    }
}
#[derive(Debug, Serialize)]
pub(crate) struct View {
    pub(crate) path: PathBuf,
    pub(crate) seq: u64,
    pub(crate) entries: Vec<Row>,
}
/// Revalidates accepted history and materialized bytes. Revocation does not erase history.
pub(crate) fn read_local(
    paths: &WorldPaths,
    scope: &str,
    members: &MemberTable,
) -> Result<View, String> {
    let path = directory(paths, scope)?;
    let seq = cursors(paths)?.get(scope).copied().unwrap_or(0);
    let mut entries = Vec::new();
    for index in 1..=seq {
        let receipt = paths
            .dir
            .join("ledger-receipts")
            .join(scope)
            .join(format!("{index:020}.json"));
        let row: Row = messages::decode(
            &std::fs::read(receipt).map_err(|e| e.to_string())?,
            "ledger receipt",
        )?;
        let candidate = row.signed.parse()?;
        let record = members
            .records
            .get(&candidate.author)
            .ok_or("Historical ledger identity missing")?;
        let author = super::members::verify_record(record)?;
        let entry = row
            .signed
            .verify_key(&crypto::b64_array(&author.node_pub, "ledger author")?)?;
        if row.seq != index || entry.scope != scope {
            return Err("Local ledger position was altered".into());
        }
        for file in &entry.files {
            let target = path.join(&file.path);
            check_ancestors(&target, &paths.dir)?;
            if std::fs::read(target).map_err(|e| e.to_string())? != file.bytes {
                return Err(format!("Local ledger file was altered: {}", file.path));
            }
        }
        entries.push(row);
    }
    Ok(View { path, seq, entries })
}
pub(crate) fn apply(paths: &WorldPaths, row: &Row, members: &MemberTable) -> Result<(), String> {
    let candidate = row.signed.parse()?;
    candidate.validate()?;
    let accepted = paths
        .dir
        .join("ledger-receipts")
        .join(&candidate.scope)
        .join(format!("{:020}.json", row.seq));
    let bytes = messages::encode(row)?;
    let already_accepted =
        super::read_optional(&accepted)?.is_some_and(|previous| previous == bytes);
    let entry = if already_accepted {
        candidate
    } else {
        row.signed.verify_history(members)?
    };
    let mut cursors = cursors(paths)?;
    let cursor = cursors.get(&entry.scope).copied().unwrap_or(0);
    if row.seq > cursor + 1 {
        return Err("ledger sequence gap".into());
    }
    let receipts = paths.dir.join("ledger-receipts").join(&entry.scope);
    std::fs::create_dir_all(&receipts).map_err(|e| e.to_string())?;
    let receipt = receipts.join(format!("{:020}.json", row.seq));
    let bytes = messages::encode(row)?;
    if let Some(previous) = super::read_optional(&receipt)?
        && previous != bytes
    {
        return Err("ledger sequence was rewritten".into());
    }
    let mut town = Town::default();
    let mut occupied = BTreeMap::new();
    for seq in 1..row.seq {
        let previous: Row = messages::decode(
            &std::fs::read(receipts.join(format!("{seq:020}.json"))).map_err(|e| e.to_string())?,
            "ledger receipt",
        )?;
        let previous = previous.signed.parse()?;
        for file in &previous.files {
            occupied.insert(file.path.to_lowercase(), previous.id.clone());
        }
        if let Some(update) = &previous.project {
            town.machines = previous.machines(update)?;
            town.project_version += 1;
        }
    }
    if row.seq == 1 && entry.project.is_none() {
        return Err("first ledger entry must establish project attributes".into());
    }
    if row.seq > 1 && !town.machines.contains(&entry.author) {
        return Err("ledger author is outside this town".into());
    }
    if let Some(update) = &entry.project
        && update.previous != town.project_version
    {
        return Err("project version conflict".into());
    }
    for file in &entry.files {
        let key = file.path.to_lowercase();
        if occupied.keys().any(|old| {
            old == &key
                || old.starts_with(&format!("{key}/"))
                || key.starts_with(&format!("{old}/"))
        }) {
            return Err("ledger path already belongs to an accepted entry".into());
        }
    }
    let root = directory(paths, &entry.scope)?;
    for file in &entry.files {
        let target = root.join(&file.path);
        check_ancestors(&target, &paths.dir)?;
        if let Some(previous) = super::read_optional(&target)?
            && previous != file.bytes
        {
            return Err(format!("ledger file conflict: {}", file.path));
        }
    }
    // Persist the accepted signed bundle before materialization. Replaying it completes a
    // crash-interrupted multi-file write; the cursor advances only after every file exists.
    create_exact(&receipt, &bytes)?;
    for file in &entry.files {
        create_exact(&root.join(&file.path), &file.bytes)?;
    }
    cursors.insert(entry.scope, cursor.max(row.seq));
    super::write_atomic(
        &paths.dir.join("ledger-cursors.json"),
        &messages::encode(&cursors)?,
    )
}
fn check_ancestors(path: &Path, boundary: &Path) -> Result<(), String> {
    for ancestor in path.ancestors().take_while(|p| *p != boundary) {
        match std::fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err("ledger path crosses a symbolic link".into());
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}
fn create_exact(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    if let Some(previous) = super::read_optional(path)? {
        return if previous == bytes {
            Ok(())
        } else {
            Err("immutable ledger bytes differ".into())
        };
    }
    let parent = path.parent().ok_or("ledger path needs a parent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    temporary
        .persist_noclobber(path)
        .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}
