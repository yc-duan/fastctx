//! Typed bodies carried inside envelopes, and the message-type names on their headers.
//!
//! Two families exist. Hub-terminated messages (`to == "hub"` or `from == "hub"`) travel as
//! plaintext bodies (epoch 0) because the hub has to read them; they carry metadata only.
//! Member-to-member messages travel encrypted under the World key; the hub routes them on
//! their headers and never opens them. The `plaintext_allowed` table below is what a
//! receiver consults before trusting a body that arrived unencrypted.

use super::envelope::Envelope;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(crate) mod kind {
    // Member → hub, reliable.
    pub(crate) const MEMBER_PUBLISH: &str = "member_publish";
    pub(crate) const INVITE_CREATE: &str = "invite_create";
    pub(crate) const KEY_PUBLISH: &str = "key_publish";
    pub(crate) const INVENTORY: &str = "inventory";
    pub(crate) const LEAVE: &str = "leave";
    // Hub → member, reliable.
    pub(crate) const GRANT_SYNC: &str = "grant_sync";
    pub(crate) const MEMBERS_CHANGED: &str = "members_changed";
    pub(crate) const KEY_ROTATED: &str = "key_rotated";
    // Requests to the hub and their answers.
    pub(crate) const INVENTORY_GET: &str = "inventory_get";
    pub(crate) const INVENTORY_RESULT: &str = "inventory_result";
    pub(crate) const MEMBERS_GET: &str = "members_get";
    pub(crate) const MEMBERS_RESULT: &str = "members_result";
    pub(crate) const EVENTS_GET: &str = "events_get";
    pub(crate) const EVENTS_RESULT: &str = "events_result";
    pub(crate) const KEYS_GET: &str = "keys_get";
    pub(crate) const KEYS_RESULT: &str = "keys_result";
    pub(crate) const GRANTS_GET: &str = "grants_get";
    /// A request rather than a reliable message: the hub accepts a snapshot only when its
    /// revision follows the stored one, and the publisher needs to hear a refusal.
    pub(crate) const GRANT_PUBLISH: &str = "grant_publish";
    pub(crate) const REVOKE: &str = "revoke";
    pub(crate) const HUB_RESULT: &str = "hub_result";
    pub(crate) const HUB_ERROR: &str = "hub_error";
    // Member ↔ member requests, encrypted.
    pub(crate) const CALL: &str = "call";
    pub(crate) const CALL_RESULT: &str = "call_result";
    pub(crate) const CANCEL: &str = "cancel";
    /// Hub-originated answer to a request the target could not receive.
    pub(crate) const CALL_STATUS: &str = "call_status";
}

/// Whether a receiver may trust a plaintext body of this type from this sender.
///
/// Every hub-originated type and every hub-terminated type is plaintext by construction.
/// Member-to-member types must arrive encrypted: a plaintext `call` would let a compromised
/// hub run tools on members.
pub(crate) fn plaintext_allowed(t: &str, from: &str) -> bool {
    if from == super::HUB_NAME {
        return true;
    }
    matches!(
        t,
        kind::MEMBER_PUBLISH
            | kind::INVITE_CREATE
            | kind::KEY_PUBLISH
            | kind::GRANT_PUBLISH
            | kind::LEAVE
            | kind::INVENTORY_GET
            | kind::MEMBERS_GET
            | kind::EVENTS_GET
            | kind::KEYS_GET
            | kind::GRANTS_GET
            | kind::REVOKE
            | kind::CANCEL
    )
}

/// Message types the hub verifies an author signature on before accepting.
pub(crate) fn signature_required(t: &str) -> bool {
    matches!(
        t,
        kind::MEMBER_PUBLISH
            | kind::INVITE_CREATE
            | kind::KEY_PUBLISH
            | kind::GRANT_PUBLISH
            | kind::REVOKE
            | kind::LEAVE
    )
}

/// The fields a revocation signs: which key, under which name, is out of the World.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct RevocationStatement {
    pub(crate) name: String,
    /// The revoked member's Ed25519 public key, base64; a revocation follows the key, so the
    /// hub cannot readmit it under another name.
    pub(crate) node_pub: String,
    /// The member that revoked; a member leaving revokes itself.
    pub(crate) by: String,
    pub(crate) at: String,
    /// `revoked` or `left`.
    pub(crate) reason: String,
}

/// A revocation as the hub stores and relays it. Members keep these forever: a revoked key is
/// never trusted again, whatever the hub later lists.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SignedRevocation {
    /// JSON text of a `RevocationStatement`, byte-exact.
    pub(crate) statement: String,
    /// Ed25519 signature by `by` over `statement` bytes (domain `revocation`), base64.
    pub(crate) sig: String,
}

impl SignedRevocation {
    pub(crate) fn parse(&self) -> Result<RevocationStatement, String> {
        serde_json::from_str(&self.statement)
            .map_err(|error| format!("the revocation is not valid JSON: {error}"))
    }
}

/// The record a member publishes about itself; MAC'd under `K_mac`, signed by the member.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct MemberRecord {
    pub(crate) name: String,
    /// Ed25519 public key, base64.
    pub(crate) node_pub: String,
    /// X25519 wrap public key, base64.
    pub(crate) wrap_pub: String,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
    pub(crate) kind: String,
    pub(crate) os: String,
    pub(crate) arch: String,
    pub(crate) version: String,
    pub(crate) enrolled_at: String,
}

/// A member record as stored by the hub and served to members: the exact JSON bytes the
/// author MAC'd, the MAC, and the author's signature over the same bytes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SignedRecord {
    /// JSON text of a `MemberRecord`, byte-exact.
    pub(crate) record: String,
    /// HMAC-SHA256 under `K_mac`, base64.
    pub(crate) mac: String,
    /// World key epoch the MAC was made under.
    pub(crate) mac_epoch: u32,
    /// Ed25519 signature by the record's `node_pub` over `record` bytes (domain
    /// `member_record`), base64.
    pub(crate) sig: String,
}

/// Body of `member_publish`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct MemberPublish {
    pub(crate) signed: SignedRecord,
}

/// Body of `invite_create`: what the hub is allowed to know about an invite.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct InviteCreate {
    pub(crate) code_id: String,
    /// `sha256(admission_token)` hex.
    pub(crate) admission: String,
    /// World keys wrapped under the invite secret, base64.
    pub(crate) wrapped_keys: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    pub(crate) exp: String,
}

/// Body of `key_publish`: one rotated epoch sealed to every remaining member.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct KeyPublish {
    pub(crate) epoch: u32,
    pub(crate) sealed: Vec<SealedKeyFor>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct SealedKeyFor {
    pub(crate) name: String,
    pub(crate) key: super::keys::SealedKey,
}

/// Body of `keys_get`: which epochs this member still lacks.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct KeysGet {
    pub(crate) have: Vec<u32>,
}

/// Body of `keys_result`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct KeysResult {
    pub(crate) sealed: Vec<super::keys::SealedKey>,
    pub(crate) newest_epoch: u32,
}

/// Body of `key_rotated` (hub → every online member).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct KeyRotated {
    pub(crate) epoch: u32,
}

/// Body of `revoke` (member → hub, a signed request) and of `leave`: the signed revocation the
/// hub stores and relays.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Revoke {
    pub(crate) revocation: SignedRevocation,
}

/// Body of `members_changed` and the version carried by `members_result`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct MembersChanged {
    pub(crate) version: u64,
}

/// Body of `members_result`. Revoked members are listed too (state `revoked`), because a
/// member must learn of a revocation and must be able to countersign one the hub operator
/// made without a World key.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct MembersResult {
    pub(crate) version: u64,
    pub(crate) members: Vec<MemberEntry>,
    /// Admitted members that hold no sealed copy of the newest World key epoch; any member
    /// holding it seals one for them.
    #[serde(default)]
    pub(crate) missing_key: Vec<String>,
    /// Newest World key epoch any member has published to the hub; 0 before any rotation.
    #[serde(default)]
    pub(crate) key_epoch: u32,
}

/// One member as the hub reports it: the signed record plus hub-side presence facts.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct MemberEntry {
    pub(crate) name: String,
    pub(crate) signed: Option<SignedRecord>,
    /// `online`, `offline`, or `revoked`.
    pub(crate) state: String,
    pub(crate) last_seen: String,
    #[serde(default)]
    pub(crate) hub_rtt_ms: Option<u32>,
    #[serde(default)]
    pub(crate) tls: Option<String>,
    #[serde(default)]
    pub(crate) network: Option<String>,
    #[serde(default)]
    pub(crate) version: Option<String>,
    #[serde(default)]
    pub(crate) inventory_version: u64,
    /// The signed revocation, when a member (rather than the hub operator) revoked this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) revocation: Option<SignedRevocation>,
}

/// Body of `inventory_get`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct InventoryGet {
    /// Members whose inventory is wanted; empty means every member.
    #[serde(default)]
    pub(crate) names: Vec<String>,
    /// Skip entries whose version is not above the caller's copy.
    #[serde(default)]
    pub(crate) have: BTreeMap<String, u64>,
}

/// Body of `inventory_result`: stored inventory envelopes, still sealed.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct InventoryResult {
    pub(crate) entries: Vec<InventoryEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct InventoryEntry {
    pub(crate) name: String,
    pub(crate) version: u64,
    pub(crate) envelope: Envelope,
}

/// Body of `events_get`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct EventsGet {
    pub(crate) since: u64,
    #[serde(default)]
    pub(crate) limit: Option<u32>,
}

/// Body of `events_result`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct EventsResult {
    pub(crate) events: Vec<Event>,
    /// The newest sequence number the hub holds, so a reader knows whether it is caught up.
    pub(crate) latest: u64,
}

/// One entry of the World's append-only log (`design-objects.md` §6). The hub writes only
/// metadata here: kinds, handles, counts, state words.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Event {
    pub(crate) seq: u64,
    pub(crate) at: String,
    pub(crate) subject: String,
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) facts: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) task: Option<String>,
}

/// Body of `hub_result`: a generic success answer from the hub.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct HubResult {
    #[serde(default)]
    pub(crate) facts: BTreeMap<String, serde_json::Value>,
}

/// Body of `hub_error`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct HubError {
    pub(crate) code: String,
    pub(crate) message: String,
}

/// The per-tool token budgets a caller carries with a call so the target renders inside
/// the caller's window.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct CallBudget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) global: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tool: Option<usize>,
}

/// Body of `call` (encrypted): a direct tool invocation on the target member.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Call {
    pub(crate) verb: String,
    /// The tool's own request object, exactly as the local tool would receive it, without
    /// the `node` selector.
    pub(crate) args: serde_json::Value,
    pub(crate) budget: CallBudget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cwd: Option<String>,
    /// Caller-side deadline in milliseconds; the target stops work after it.
    pub(crate) timeout_ms: u64,
    /// The grant snapshot revision the caller holds. It travels inside the ciphertext, so a
    /// target that is behind learns it from a source the hub cannot lower.
    #[serde(default)]
    pub(crate) grant_revision: u64,
}

/// Body of `call_result` (encrypted): the target's rendered response.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CallResult {
    pub(crate) node: String,
    pub(crate) response: WireResponse,
    pub(crate) elapsed_ms: u64,
    /// The grant snapshot revision the target holds (see `Call::grant_revision`).
    #[serde(default)]
    pub(crate) grant_revision: u64,
}

/// A `ToolResponse` on the wire.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct WireResponse {
    pub(crate) content: Vec<WireContent>,
    pub(crate) is_error: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum WireContent {
    Text {
        text: String,
    },
    Image {
        data: String,
        mime_type: String,
        #[serde(default)]
        high_detail: bool,
    },
}

impl From<&crate::model::ToolResponse> for WireResponse {
    fn from(response: &crate::model::ToolResponse) -> Self {
        Self {
            content: response
                .content
                .iter()
                .map(|block| match block {
                    crate::model::ToolContent::Text(text) => {
                        WireContent::Text { text: text.clone() }
                    }
                    crate::model::ToolContent::Image {
                        data,
                        mime_type,
                        detail,
                    } => WireContent::Image {
                        data: data.clone(),
                        mime_type: mime_type.clone(),
                        high_detail: detail.is_some(),
                    },
                })
                .collect(),
            is_error: response.is_error,
        }
    }
}

impl From<WireResponse> for crate::model::ToolResponse {
    fn from(response: WireResponse) -> Self {
        Self {
            content: response
                .content
                .into_iter()
                .map(|block| match block {
                    WireContent::Text { text } => crate::model::ToolContent::Text(text),
                    WireContent::Image {
                        data,
                        mime_type,
                        high_detail,
                    } => crate::model::ToolContent::Image {
                        data,
                        mime_type,
                        detail: high_detail.then_some(crate::model::ImageDetail::High),
                    },
                })
                .collect(),
            is_error: response.is_error,
        }
    }
}

/// Body of `call_status` (hub → caller, plaintext): what happened to one target's leg.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CallStatus {
    pub(crate) node: String,
    /// `delivered` (the hub handed the call to an online target; an answer follows), or a
    /// terminal `offline`, `unknown`, `forbidden`, `revoked`, or `disconnected`.
    pub(crate) status: String,
    pub(crate) message: String,
}

/// Status a `call_status` carries when the leg is under way rather than finished.
pub(crate) const CALL_DELIVERED: &str = "delivered";

/// Body of `grant_sync` (hub → member, and the answer to `grants_get`): the snapshot in
/// force, or none when no member has published one.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct GrantSync {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) set: Option<super::grant::SignedGrantSet>,
}

/// Body of `grant_publish` (member → hub, a signed request).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct GrantPublish {
    pub(crate) set: super::grant::SignedGrantSet,
}

pub(crate) fn encode<T: Serialize>(body: &T) -> Result<Vec<u8>, String> {
    serde_json::to_vec(body).map_err(|error| format!("Cannot encode a World message body: {error}"))
}

pub(crate) fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8], t: &str) -> Result<T, String> {
    serde_json::from_slice(bytes).map_err(|error| format!("Malformed {t} body: {error}"))
}
