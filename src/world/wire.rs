//! WebSocket frames. Application envelopes carry mandatory author signatures; transport ids correlate delivery.

use super::envelope::Envelope;
use serde::{Deserialize, Serialize};

/// The 256 KiB control budget includes the serialized envelope and transport fields.
pub(crate) const MAX_FRAME_BYTES: usize = super::envelope::MAX_CONTROL_MESSAGE_BYTES;

/// Which end of a `copy` data connection a member is.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StreamRole {
    Source,
    Sink,
}

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
    /// One end of a `copy` data connection: authenticated exactly like `auth`, then paired by
    /// `stream` with the other end and pumped. Never a session, never replaces one.
    Stream { stream: u64, role: StreamRole },
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
    /// Proves knowledge of the bootstrap verifier before the founding device sends admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) bootstrap_proof: Option<String>,
}

/// Enrollment material presented with `auth` when the intent is `enroll` or `bootstrap`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Enrollment {
    /// One-time invitation public key, or `bootstrap` for the founding device.
    pub(crate) code_id: String,
    /// Bootstrap bearer only; invited devices use their admission signature instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) admission_token: Option<String>,
    pub(crate) name: String,
    pub(crate) record: super::messages::SignedRecord,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
}

pub(crate) fn bootstrap_transcript(handshake: &[u8], world: &str) -> Vec<u8> {
    let mut bytes = super::SIGNATURE_DOMAIN_PREFIX.as_bytes().to_vec();
    bytes.extend_from_slice(b"bootstrap_hub\0");
    bytes.extend_from_slice(&(world.len() as u64).to_be_bytes());
    bytes.extend_from_slice(world.as_bytes());
    bytes.extend_from_slice(handshake);
    bytes
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Auth {
    /// `Sign(node, "node", nonce_h ‖ nonce_n ‖ node_pub ‖ hub_pub ‖ binding)`, base64.
    pub(crate) sig: String,
    /// Highest hub sequence number this member has processed; the hub resends above it.
    pub(crate) recv_seq: u64,
    pub(crate) ticket: Option<Box<super::stream_ticket::Ticket>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) enrollment: Option<Box<Enrollment>>,
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
    /// Both ends of a data connection are in place; what follows on this connection is the
    /// stream's records, not frames.
    Attached {
        stream: u64,
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
        let bytes = serde_json::to_vec(self)
            .map_err(|error| format!("Cannot encode a World frame: {error}"))?;
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(format!(
                "frame_too_large: {} > {MAX_FRAME_BYTES}",
                bytes.len()
            ));
        }
        Ok(bytes)
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
