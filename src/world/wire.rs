//! Transport frames between a member and the hub.
//!
//! One WebSocket binary message carries one JSON `Frame`. The link vocabulary (handshake,
//! heartbeat, replacement, acks) lives here in plaintext because the hub is a party to it;
//! everything applications say travels inside an `Envelope` in a `msg` frame, where the hub
//! sees the header and, for content addressed to members, nothing else.
//!
//! Reliability is a transport property: a `msg` frame with `seq` is stored by its sender
//! until the peer's `ack` names that `seq`, and every direction numbers its own sequence.
//! Request correlation (`id`) is likewise per direction; the hub renumbers a request it
//! forwards so that a target's answer can be matched back to the caller.

use super::envelope::Envelope;
use serde::{Deserialize, Serialize};

/// Largest frame accepted on the control connection.
pub(crate) const MAX_FRAME_BYTES: usize = super::envelope::MAX_CONTROL_MESSAGE_BYTES + 4096;

/// Client-side intent declared in `hello`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Intent {
    /// An enrolled member reconnecting.
    Auth,
    /// A new machine presenting an invite.
    Enroll,
    /// The first machine of a World presenting the hub's bootstrap password.
    Bootstrap,
}

/// How the hub binds the application handshake to the TLS connection.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BindingMode {
    /// RFC 9266 `tls-exporter`; both ends terminate the same TLS connection.
    Exporter,
    /// A proxy or CDN terminates TLS in front of the hub; the binding is empty.
    None,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Hello {
    pub(crate) protocol: u32,
    pub(crate) min_protocol: u32,
    /// fastctx version of the member, for diagnostics only.
    pub(crate) version: String,
    /// 32 random bytes, base64.
    pub(crate) nonce: String,
    /// Member Ed25519 public key, base64.
    pub(crate) node_pub: String,
    /// Member X25519 wrap public key, base64.
    pub(crate) wrap_pub: String,
    pub(crate) intent: Intent,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Challenge {
    pub(crate) protocol: u32,
    /// 32 random bytes, base64.
    pub(crate) nonce: String,
    /// Hub Ed25519 public key, base64.
    pub(crate) hub_pub: String,
    pub(crate) world_id: String,
    pub(crate) binding: BindingMode,
    /// `Sign(hub, "hub", nonce_n ‖ nonce_h ‖ hub_pub ‖ node_pub ‖ binding)`, base64.
    pub(crate) sig: String,
}

/// Enrollment material presented with `auth` when the intent is `enroll` or `bootstrap`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Enrollment {
    /// `sha256("invite" ‖ secret)` hex, or `bootstrap` for the first member.
    pub(crate) code_id: String,
    /// The one-time bearer, base64.
    pub(crate) admission_token: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Auth {
    /// `Sign(node, "node", nonce_h ‖ nonce_n ‖ node_pub ‖ hub_pub ‖ binding)`, base64.
    pub(crate) sig: String,
    /// Highest hub sequence number this member has processed; the hub resends above it.
    pub(crate) recv_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) enrollment: Option<Enrollment>,
}

/// Material a freshly enrolled member receives once.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Enrolled {
    /// The World keys wrapped under the invite secret; absent for the bootstrap member, who
    /// creates the first epoch itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) wrapped_keys: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Welcome {
    pub(crate) world_id: String,
    /// The name the hub knows this member by.
    pub(crate) name: String,
    pub(crate) hub_time: String,
    /// Highest member sequence number the hub has processed; the member resends above it.
    pub(crate) recv_seq: u64,
    /// Highest sequence number the hub has assigned towards this member.
    pub(crate) send_seq: u64,
    pub(crate) members_version: u64,
    pub(crate) grant_version: u64,
    /// Newest World key epoch any member has published to the hub.
    pub(crate) key_epoch: u32,
    /// A revocation happened while no member could rotate the key; the first member to see
    /// this completes the rotation.
    pub(crate) rotation_pending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) enrolled: Option<Enrolled>,
}

/// Why the hub refused a connection. `code` is one of the `design-transport.md` §15 names.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Rejected {
    pub(crate) code: String,
    pub(crate) message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) protocol: Option<ProtocolMismatch>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub(crate) struct ProtocolMismatch {
    pub(crate) ours: u32,
    pub(crate) theirs: u32,
}

/// Light facts a heartbeat carries; the hub sees them (they are presence, not content).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Load {
    #[serde(default)]
    pub(crate) cpu_pct: u8,
    #[serde(default)]
    pub(crate) mem_free_gb: f32,
    #[serde(default)]
    pub(crate) running_steps: u32,
    #[serde(default)]
    pub(crate) outbox_depth: u32,
    /// The member's inventory version, so the hub can notice a stale copy.
    #[serde(default)]
    pub(crate) facts_version: u64,
    /// The member's measured round trip to the hub, for the machine map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rtt_ms: Option<u32>,
    /// `direct` or `system`: which path this connection took.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) network: Option<String>,
    /// `webpki`, `pinned`, or `fronted`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tls: Option<String>,
}

/// One WebSocket message.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "f", rename_all = "snake_case")]
pub(crate) enum Frame {
    Hello(Hello),
    Challenge(Challenge),
    Auth(Auth),
    Welcome(Welcome),
    Rejected(Rejected),
    Heartbeat {
        seq: u64,
        load: Load,
    },
    HeartbeatAck {
        seq: u64,
        hub_time: String,
    },
    /// Another connection authenticated as the same member; this one is being closed.
    Replaced,
    Bye {
        reason: String,
    },
    /// An application envelope. `seq` makes it reliable; `id` correlates a request or its
    /// answer; neither means fire-and-forget.
    Msg {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<u64>,
        env: Envelope,
    },
    Ack {
        seq: u64,
    },
}

impl Frame {
    pub(crate) fn encode(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self).map_err(|error| format!("Cannot encode a World frame: {error}"))
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(format!(
                "A {}-byte World frame exceeds the {MAX_FRAME_BYTES}-byte limit.",
                bytes.len()
            ));
        }
        serde_json::from_slice(bytes)
            .map_err(|error| format!("Cannot parse a World frame: {error}"))
    }

    pub(crate) fn reliable(seq: u64, env: Envelope) -> Self {
        Self::Msg {
            seq: Some(seq),
            id: None,
            env,
        }
    }

    pub(crate) fn request(id: u64, env: Envelope) -> Self {
        Self::Msg {
            seq: None,
            id: Some(id),
            env,
        }
    }
}

/// The bytes both sides sign during the handshake, in the order the hub and node sign them.
pub(crate) fn hub_transcript(
    node_nonce: &[u8],
    hub_nonce: &[u8],
    hub_pub: &[u8],
    node_pub: &[u8],
    binding: &[u8],
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(node_nonce.len() + hub_nonce.len() + 64 + binding.len());
    bytes.extend_from_slice(node_nonce);
    bytes.extend_from_slice(hub_nonce);
    bytes.extend_from_slice(hub_pub);
    bytes.extend_from_slice(node_pub);
    bytes.extend_from_slice(binding);
    bytes
}

pub(crate) fn node_transcript(
    hub_nonce: &[u8],
    node_nonce: &[u8],
    node_pub: &[u8],
    hub_pub: &[u8],
    binding: &[u8],
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(node_nonce.len() + hub_nonce.len() + 64 + binding.len());
    bytes.extend_from_slice(hub_nonce);
    bytes.extend_from_slice(node_nonce);
    bytes.extend_from_slice(node_pub);
    bytes.extend_from_slice(hub_pub);
    bytes.extend_from_slice(binding);
    bytes
}

/// Signature domain of the hub's `challenge`.
pub(crate) const HUB_HANDSHAKE_DOMAIN: &str = "hub";
/// Signature domain of the member's `auth`.
pub(crate) const NODE_HANDSHAKE_DOMAIN: &str = "node";
/// TLS exporter label for the channel binding (RFC 9266).
pub(crate) const EXPORTER_LABEL: &[u8] = b"EXPORTER-Channel-Binding";
/// Length of the exported binding.
pub(crate) const BINDING_LEN: usize = 32;
