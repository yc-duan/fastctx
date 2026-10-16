//! Conflicting legacy letter bytes remain outside the active mailbox forever.

use super::*;

pub(super) fn keep(
    dir: &Path,
    epoch: u64,
    seq: u64,
    old_path: &Path,
    old: &[u8],
    new_path: &Path,
    new: &[u8],
) -> io::Result<PathBuf> {
    let directory = dir.join("outbox-conflicts").join(format!("{epoch:016x}")).join(format!("{seq:020}"));
    ensure_private_dir(&directory)?;
    let keep = |bytes: &[u8]| -> io::Result<PathBuf> {
        let digest: String = fastctx_net::sign::sha256(bytes).iter().map(|byte| format!("{byte:02x}")).collect();
        let path = directory.join(format!("{digest}.json"));
        match std::fs::read(&path) {
            Ok(kept) if kept != bytes => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("conflict archive has different bytes: {}", path.display()),
                ));
            }
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        publish(&path, bytes, Permissions::Private)?;
        Ok(path)
    };
    let old_copy = keep(old)?;
    let new_copy = keep(new)?;
    let record = directory.join("conflict.json");
    publish(
        &record,
        &json(&serde_json::json!({
            "reason": "legacy and current mailboxes contain different bytes for the same number",
            "legacy_source": old_path,
            "current_source": new_path,
            "legacy_copy": old_copy,
            "current_copy": new_copy,
        }))?,
        Permissions::Private,
    )?;
    Ok(record)
}
