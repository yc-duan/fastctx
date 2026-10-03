//! Every application envelope is signed over its exact header bytes and body hash.
use super::crypto::{self, b64_decode, b64_encode, b64url_decode, b64url_encode};
use super::identity::{self, Signature64, SigningIdentity};
use serde::{Deserialize, Serialize};

/// Largest control envelope on the wire (`design-transport.md` §5.1).
pub(crate) const MAX_CONTROL_MESSAGE_BYTES: usize = 256 * 1024;

/// Plaintext routing header. Every field the hub acts on lives here; nothing else does.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Header {
    /// Message type.
    pub(crate) t: String,
    /// Sending member name, or `hub`.
    pub(crate) from: String,
    /// Receiving member name, or `hub`.
    pub(crate) to: String,
    /// The sender's monotonic message counter; receivers refuse counters already accepted.
    pub(crate) n: u64,
    /// Request correlation number for request/response messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<u64>,
    /// Tool verb of a call, visible so the hub can apply grant shapes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) verb: Option<String>,
    /// Every addressee of a message sent to several members at once; absent when `to` is the
    /// only one. The tool surface sends to one member; the field stays for primitives that are
    /// multi-machine by nature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) targets: Option<Vec<String>>,
    /// Seconds the caller of a request keeps waiting for its answer; the hub keeps the pending
    /// entry that long. A script behind the command line may wait hours where a host waits
    /// minutes; the signed routing header carries the same declared window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) wait_s: Option<u64>,
}

impl Header {
    /// A header for a message routed member-to-member or member-to-hub.
    pub(crate) fn new(t: &str, from: &str, to: &str, n: u64) -> Self {
        Self {
            t: t.to_string(),
            from: from.to_string(),
            to: to.to_string(),
            n,
            id: None,
            verb: None,
            targets: None,
            wait_s: None,
        }
    }

    pub(crate) fn with_id(mut self, id: u64) -> Self {
        self.id = Some(id);
        self
    }

    pub(crate) fn with_verb(mut self, verb: &str) -> Self {
        self.verb = Some(verb.to_string());
        self
    }

    pub(crate) fn with_wait(mut self, seconds: u64) -> Self {
        self.wait_s = Some(seconds);
        self
    }
}

/// One signed application message.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Envelope {
    /// base64url of the header JSON bytes; those exact bytes are the AEAD additional data.
    pub(crate) hdr: String,
    /// Base64 of the exact body JSON bytes.
    pub(crate) body: String,
    /// base64 Ed25519 signature by `from`, when the message type requires authorship.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sig: Option<String>,
}

/// Header and body decoded after the caller verifies the signature.
#[derive(Clone, Debug)]
pub(crate) struct Opened {
    pub(crate) header: Header,
    pub(crate) body: Vec<u8>,
}

impl Envelope {
    pub(crate) fn seal_plain(header: Header, body: &[u8]) -> Result<Self, String> {
        Ok(Self {
            hdr: b64url_encode(&header_bytes(&header)?),
            body: b64_encode(body),
            sig: None,
        })
    }
    /// Decodes the header without touching the body.
    pub(crate) fn header(&self) -> Result<Header, String> {
        let bytes = self.header_bytes()?;
        serde_json::from_slice(&bytes).map_err(|error| format!("Invalid envelope header: {error}"))
    }

    pub(crate) fn header_bytes(&self) -> Result<Vec<u8>, String> {
        b64url_decode(&self.hdr).map_err(|error| format!("Invalid envelope header: {error}"))
    }

    pub(crate) fn body_bytes(&self) -> Result<Vec<u8>, String> {
        b64_decode(&self.body).map_err(|error| format!("Invalid envelope body: {error}"))
    }

    pub(crate) fn open(&self) -> Result<Opened, String> {
        Ok(Opened {
            header: self.header()?,
            body: self.body_bytes()?,
        })
    }
    /// Signs the envelope as its `from` member under the message type's domain.
    pub(crate) fn sign(&mut self, identity: &SigningIdentity) -> Result<(), String> {
        let header = self.header()?;
        let signature = identity.sign(&header.t, &self.signed_bytes()?);
        self.sig = Some(b64_encode(&signature));
        Ok(())
    }

    /// Verifies the signature against the public key the caller resolved for `from`.
    pub(crate) fn verify_signature(&self, public_key: &crypto::Key32) -> Result<(), String> {
        let header = self.header()?;
        let signature = self
            .sig
            .as_deref()
            .ok_or_else(|| format!("The {} message carries no signature.", header.t))?;
        let signature: Signature64 = crypto::b64_array(signature, "envelope signature")?;
        identity::verify(public_key, &header.t, &self.signed_bytes()?, &signature)
    }

    fn signed_bytes(&self) -> Result<Vec<u8>, String> {
        let mut bytes = self.header_bytes()?;
        bytes.extend_from_slice(&crypto::sha256(&self.body_bytes()?));
        Ok(bytes)
    }
}

fn header_bytes(header: &Header) -> Result<Vec<u8>, String> {
    serde_json::to_vec(header)
        .map_err(|error| format!("Cannot encode the envelope header: {error}"))
}
