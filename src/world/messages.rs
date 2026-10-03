use super::envelope::Envelope;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(crate) mod kind {
    pub(crate) const DELIVERY_REJECTED: &str = "delivery_rejected";
    pub(crate) const CALL_RESULT_STREAM: &str = "call_result_stream";
    pub(crate) const SUBSCRIBE: &str = "subscribe";
    pub(crate) const UNSUBSCRIBE: &str = "unsubscribe";
    pub(crate) const SUBSCRIPTION_EVENT: &str = "subscription_event";
    pub(crate) const SUBSCRIPTION_CLOSED: &str = "subscription_closed";
    pub(crate) const LEDGER_APPEND: &str = "ledger_append";
    pub(crate) const LEDGER_TOWNS: &str = "ledger_towns";
    pub(crate) const LEDGER_NEXT: &str = "ledger_next";
    pub(crate) const LEDGER_CHANGED: &str = "ledger_changed";
    pub(crate) const MAIL: &str = "mail";
    // Member → hub, reliable.
    pub(crate) const MEMBER_PUBLISH: &str = "member_publish";
    pub(crate) const INVITE_CREATE: &str = "invite_create";
    pub(crate) const INVENTORY: &str = "inventory";
    pub(crate) const LEAVE: &str = "leave";
    // Hub → member, reliable.
    pub(crate) const GRANT_SYNC: &str = "grant_sync";
    pub(crate) const MEMBERS_CHANGED: &str = "members_changed";
    // Requests to the hub and their answers.
    pub(crate) const INVENTORY_GET: &str = "inventory_get";
    pub(crate) const INVENTORY_RESULT: &str = "inventory_result";
    pub(crate) const MEMBERS_GET: &str = "members_get";
    pub(crate) const MEMBERS_RESULT: &str = "members_result";
    pub(crate) const EVENTS_GET: &str = "events_get";
    pub(crate) const EVENTS_RESULT: &str = "events_result";
    pub(crate) const GRANTS_GET: &str = "grants_get";
    /// A request rather than a reliable message: the hub accepts a snapshot only when its
    /// revision follows the stored one, and the publisher needs to hear a refusal.
    pub(crate) const GRANT_PUBLISH: &str = "grant_publish";
    pub(crate) const REVOKE: &str = "revoke";
    pub(crate) const HUB_RESULT: &str = "hub_result";
    pub(crate) const HUB_ERROR: &str = "hub_error";
    // Member ↔ member requests, signed.
    pub(crate) const CALL: &str = "call";
    pub(crate) const CALL_RESULT: &str = "call_result";
    pub(crate) const CANCEL: &str = "cancel";
    /// Hub-originated answer to a request the target could not receive.
    pub(crate) const CALL_STATUS: &str = "call_status";
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

/// The record a member signs about itself.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct MemberRecord {
    pub(crate) name: String,
    /// Ed25519 public key, base64.
    pub(crate) node_pub: String,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
    pub(crate) kind: String,
    pub(crate) os: String,
    pub(crate) arch: String,
    pub(crate) version: String,
    pub(crate) enrolled_at: String,
}

/// A member record as stored by the hub and served to members: the exact JSON bytes the
/// author signed, together with its admission credential.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SignedRecord {
    pub(crate) admission: super::admission::Credential,
    /// JSON text of a `MemberRecord`, byte-exact.
    pub(crate) record: String,
    /// Ed25519 signature by the record's `node_pub` over `record` bytes (domain
    /// `member_record`), base64.
    pub(crate) sig: String,
}

/// Body of `member_publish`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct MemberPublish {
    pub(crate) signed: SignedRecord,
}

/// The inviter's endorsement; no invitation seed reaches the hub.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct InviteCreate {
    pub(crate) endorsement: super::admission::Endorsement,
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
/// member must retain signed revocations alongside historical admission records.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct MembersResult {
    pub(crate) version: u64,
    pub(crate) members: Vec<MemberEntry>,
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

/// Body of `inventory_result`: stored inventory envelopes, still signed.
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

/// One entry of the World's append-only log. The hub writes only
/// metadata here: kinds, handles, counts, state words.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Event {
    pub(crate) seq: u64,
    pub(crate) at: String,
    pub(crate) subject: String,
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) facts: BTreeMap<String, serde_json::Value>,
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

/// Who waits for a call: a host's tool call, bounded by the host's own tool timeout, or a
/// script behind `fastctx world run` / `copy`, which may wait hours. The target relaxes its
/// per-call ceilings for the second (`design-transport.md` §8).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CallOrigin {
    #[default]
    Tool,
    Script,
}

/// Body of `call` (signed): a direct tool invocation on the target member.
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
    /// The grant snapshot revision the caller holds. It travels inside the signed body, so a
    /// target that is behind learns it from a source the hub cannot lower.
    #[serde(default)]
    pub(crate) grant_revision: u64,
    #[serde(default)]
    pub(crate) origin: CallOrigin,
}

/// Body of `call_result` (signed): the target's rendered response.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CallResult {
    pub(crate) node: String,
    pub(crate) response: WireResponse,
    pub(crate) elapsed_ms: u64,
    /// The grant snapshot revision the target holds (see `Call::grant_revision`).
    #[serde(default)]
    pub(crate) grant_revision: u64,
    /// Structured facts beside the rendered response: `exit_code` and `cwd` of a `run`, the
    /// `sha256` and canonical `path` of a file an inspect or a write touched. Scripts and the
    /// caller's session read these; the model reads the head note.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) facts: BTreeMap<String, serde_json::Value>,
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
