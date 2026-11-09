//! Legacy plan submission compatibility over the shared text merge algorithm.

/// Legacy and disconnected submissions preserve a conflicting incoming file as
/// the next revision; the previous hub revision remains in the archive.
pub fn merge(base: &str, hub: &str, file: &str) -> String {
    try_merge(base, hub, file).unwrap_or_else(|| file.into())
}

/// Merges independent line changes. An overlap is a conflict, not permission
/// to overwrite the hub. Concurrent appends retain the hub's arrival first.
pub fn try_merge(base: &str, hub: &str, file: &str) -> Option<String> {
    fastctx_types::write::merge::try_merge(base, hub, file)
}

/// The submission identity is determined by the original edit, not by a
/// merged result or by a retry time. Account incarnation scopes its receipt.
pub fn submission(owner: &fastctx_types::Address, base: Option<u64>, markdown: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(serde_json::to_vec(&(owner, base, markdown)).expect("plain values serialize")))
}
