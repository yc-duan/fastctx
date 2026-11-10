//! Stable conversation coordinates shared by ingestion and legacy import.
//!
//! V1 hashes an ordered sequence of fields with SHA-256, retaining all 32 bytes.
//! Every field, including the domain and each variant tag, is prefixed by its
//! byte length as an unsigned big-endian u64. The first two fields are
//! `fastctx.conversation-identity.v1` and the entity domain. Strings are exact
//! UTF-8 without normalization; existing identifiers include their canonical
//! textual prefixes. Indexes use eight big-endian bytes. Birth timestamps use
//! signed big-endian i128 nanoseconds since the Unix epoch, independent of the
//! timestamp's input timezone. Field order is defined by each key's encoder.
//!
//! Ownership, accounts, runtime paths, host-file generations, branch selection
//! and body bytes are not identity inputs. A birth's device and timestamp must
//! come from verified, immutable birth evidence, never the current location.
//! The birth timestamp must be the original value in the authoritative birth
//! record, retaining all of its fractional precision. Legacy import uses only
//! the birth seal's timestamp; it must not substitute, round, enrich or rebuild
//! it from timestamps elsewhere, even when they describe the same birth.
//! Missing evidence is an unresolved mapping, not a reason to invent a birth.
//! These functions allocate neither permissions nor production authority.
//!
//! The cross-package contract samples are in
//! `crates/fastctx-types/contracts/conversation-identity-v1.json`. Their preimages
//! and expected identifiers were independently computed with Python's hashlib.

mod encoding;
mod evidence;
mod keys;
mod native;
mod source;
mod sources;
pub use source::source_object;
pub use sources::{canonical_timeline_source, claude_transcript_source};

pub use evidence::{AliasBinding, AliasSource, ReplayEvidence, ReplayMatch};
pub use keys::{
    ConversationKey, ConversationObject, ItemKey, ItemRole, ProvisionalKey, RelationKey, RelationRole, TurnKey,
    conversation_scope,
};
pub use native::{
    ImageSelector, LegacyDigest, NATIVE_ID_BYTES, NativeAgentKey, NativeId, NativeItemKey, NativeTurnKey,
};

/// An identity cannot be constructed or two observations cannot be coalesced.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    #[error("a native identity must not be empty")]
    EmptyNativeId,
    #[error("a native identity must not exceed 4096 UTF-8 bytes")]
    NativeIdTooLong,
    #[error("a legacy event digest must be exactly 20 lowercase hex digits")]
    InvalidLegacyDigest,
    #[error("an alias must stay within one canonical conversation")]
    AliasConversation,
    #[error("an alias must retain its semantic role")]
    AliasRole,
    #[error("an alias source must be provisional or explicitly legacy")]
    AliasSource,
    #[error("an alias target must have a final native identity")]
    AliasTarget,
    #[error("one operation names different items")]
    ReplayItem,
    #[error("one operation and item have different body digests")]
    ReplayBody,
}
