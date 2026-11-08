//! Stable feed anchors, independent of a peer's negotiated visibility.

use super::{
    catalog::CatalogChange,
    identity::{Digest, FeedEpoch, ScopeKey},
};
use sha2::{Digest as _, Sha256};

/// Sequence zero binds the chain to one hub, scope and feed incarnation.
pub fn genesis(scope: &ScopeKey, epoch: &FeedEpoch) -> Digest {
    let mut hash = Sha256::new();
    hash.update(b"fastctx-sync/feed-genesis/v1\0");
    hash.update(scope.hub.bytes());
    hash.update(scope.scope.as_str().as_bytes());
    hash.update(epoch.as_str().as_bytes());
    Digest::from_bytes(hash.finalize().into())
}

/// The writer persists the canonical change and this anchor in the same
/// transaction. Filtering must forward this anchor, never hash a projection.
pub fn advance(previous: &Digest, change: &CatalogChange) -> Result<(Vec<u8>, Digest), String> {
    let canonical = canonical_change(change)?;
    let mut hash = Sha256::new();
    hash.update(b"fastctx-sync/feed-prefix/v1\0");
    hash.update(previous.bytes());
    hash.update(&canonical);
    Ok((canonical, Digest::from_bytes(hash.finalize().into())))
}

/// V1 uses compact UTF-8 JSON, recursively sorted object keys in UTF-8 byte
/// order, unchanged array order, integer decimal numbers and serde_json string
/// escaping. Floating-point values are excluded from catalog changes.
pub fn canonical_change(change: &CatalogChange) -> Result<Vec<u8>, String> {
    super::canonical::encode(change)
}
