//! Envelopes: the unit of application-level messaging in a World.
//!
//! An envelope is `{ hdr, body, sig? }`. The header is plaintext so the hub can route on it;
//! the body is either plaintext (epoch 0, messages the hub itself reads) or XChaCha20-Poly1305
//! ciphertext under the World key of the named epoch with the header bytes as additional
//! data. A signature, when present, covers the header bytes and a hash of the body under a
//! domain named after the message type, so the hub can verify authorship without reading
//! content and a recipient can refuse anything the hub might have fabricated.

use super::crypto::{self, b64_decode, b64_encode, b64url_decode, b64url_encode};
use super::identity::{self, Signature64, SigningIdentity};
use super::keys::KeyRing;
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
    /// The sender's monotonic message counter; receivers refuse anything at or below the
    /// highest value they have accepted from that sender.
    pub(crate) n: u64,
    /// Request correlation number for request/response messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) step: Option<u64>,
    /// Tool verb of a call or step, visible so the hub can apply grant shapes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) verb: Option<String>,
    /// Target member names of a fan-out call or step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) targets: Option<Vec<String>>,
    /// World key epoch the body is sealed under; 0 means the body is plaintext.
    pub(crate) epoch: u32,
    /// Random 24-byte nonce, base64.
    pub(crate) nonce: String,
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
            task: None,
            step: None,
            verb: None,
            targets: None,
            epoch: 0,
            nonce: String::new(),
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

    pub(crate) fn with_targets(mut self, targets: Vec<String>) -> Self {
        self.targets = Some(targets);
        self
    }

    pub(crate) fn with_task(mut self, task: &str, step: Option<u64>) -> Self {
        self.task = Some(task.to_string());
        self.step = step;
        self
    }
}

/// One sealed message.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Envelope {
    /// base64url of the header JSON bytes; those exact bytes are the AEAD additional data.
    pub(crate) hdr: String,
    /// base64 of the body: plaintext JSON (epoch 0) or ciphertext.
    pub(crate) body: String,
    /// base64 Ed25519 signature by `from`, when the message type requires authorship.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sig: Option<String>,
}

/// A decoded envelope: header plus body bytes and whether they were encrypted.
#[derive(Clone, Debug)]
pub(crate) struct Opened {
    pub(crate) header: Header,
    pub(crate) body: Vec<u8>,
    pub(crate) encrypted: bool,
}

impl Envelope {
    /// Seals a plaintext body: only for messages the hub reads (`to == "hub"` or `from == "hub"`).
    pub(crate) fn seal_plain(mut header: Header, body: &[u8]) -> Result<Self, String> {
        header.epoch = 0;
        header.nonce = b64_encode(&crypto::random_bytes::<{ crypto::NONCE_LEN }>()?);
        let hdr_bytes = header_bytes(&header)?;
        Ok(Self {
            hdr: b64url_encode(&hdr_bytes),
            body: b64_encode(body),
            sig: None,
        })
    }

    /// Seals `body` under the ring's current epoch, binding the header.
    pub(crate) fn seal(header: Header, body: &[u8], keys: &KeyRing) -> Result<Self, String> {
        Self::seal_with_epoch(header, body, keys, keys.current().epoch())
    }

    pub(crate) fn seal_with_epoch(
        mut header: Header,
        body: &[u8],
        keys: &KeyRing,
        epoch: u32,
    ) -> Result<Self, String> {
        let subkeys = keys.subkeys(epoch).ok_or_else(|| {
            format!("key_epoch_unknown: this member holds no World key for epoch {epoch}.")
        })?;
        let nonce = crypto::random_bytes::<{ crypto::NONCE_LEN }>()?;
        header.epoch = epoch;
        header.nonce = b64_encode(&nonce);
        let hdr_bytes = header_bytes(&header)?;
        let ciphertext = crypto::seal_with_nonce(&subkeys.msg, &nonce, &hdr_bytes, body)?;
        Ok(Self {
            hdr: b64url_encode(&hdr_bytes),
            body: b64_encode(&ciphertext),
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

    /// Opens the body: decrypts when the header names an epoch, passes plaintext through
    /// when the epoch is 0. The caller decides whether plaintext is acceptable for the type.
    pub(crate) fn open(&self, keys: Option<&KeyRing>) -> Result<Opened, String> {
        let hdr_bytes = self.header_bytes()?;
        let header: Header = serde_json::from_slice(&hdr_bytes)
            .map_err(|error| format!("Invalid envelope header: {error}"))?;
        let body = self.body_bytes()?;
        if header.epoch == 0 {
            return Ok(Opened {
                header,
                body,
                encrypted: false,
            });
        }
        let keys = keys.ok_or_else(|| {
            "This process holds no World key and cannot open an encrypted envelope.".to_string()
        })?;
        let subkeys = keys.subkeys(header.epoch).ok_or_else(|| {
            format!(
                "key_epoch_unknown: this member holds no World key for epoch {}.",
                header.epoch
            )
        })?;
        let nonce = crypto::b64_array::<{ crypto::NONCE_LEN }>(&header.nonce, "envelope nonce")?;
        let plain = crypto::open(&subkeys.msg, &nonce, &hdr_bytes, &body)?;
        Ok(Opened {
            header,
            body: plain,
            encrypted: true,
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

    /// Serialized size in bytes, for the control-message limit.
    pub(crate) fn wire_len(&self) -> usize {
        self.hdr.len() + self.body.len() + self.sig.as_ref().map_or(0, String::len) + 32
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

#[cfg(test)]
mod tests {
    use super::{Envelope, Header};
    use crate::world::identity::SigningIdentity;
    use crate::world::keys::KeyRing;

    #[test]
    fn encrypted_envelopes_bind_their_header_and_open_only_with_the_named_epoch() {
        let mut keys = KeyRing::new_initial().unwrap();
        let header = Header::new("call", "desktop", "laptop", 7)
            .with_id(3)
            .with_verb("grep")
            .with_targets(vec!["laptop".to_string()]);
        let envelope = Envelope::seal(header.clone(), b"{\"pattern\":\"x\"}", &keys).unwrap();
        let opened = envelope.open(Some(&keys)).unwrap();
        assert!(opened.encrypted);
        assert_eq!(opened.header.t, "call");
        assert_eq!(opened.header.n, 7);
        assert_eq!(opened.header.epoch, 1);
        assert_eq!(opened.body, b"{\"pattern\":\"x\"}");

        // A hub that rewrites the header (say, the target) cannot keep the body opening.
        let mut forged = envelope.clone();
        let mut forged_header = envelope.header().unwrap();
        forged_header.to = "attacker".to_string();
        forged.hdr =
            crate::world::crypto::b64url_encode(&serde_json::to_vec(&forged_header).unwrap());
        assert!(forged.open(Some(&keys)).is_err());

        // Without the epoch the message cannot be read; a newer epoch on the ring does not help.
        let other = KeyRing::new_initial().unwrap();
        assert!(envelope.open(Some(&other)).is_err());
        keys.rotate().unwrap();
        assert!(envelope.open(Some(&keys)).is_ok());
        assert!(Envelope::seal_with_epoch(header, b"x", &keys, 9).is_err());
        assert!(envelope.open(None).is_err());
    }

    #[test]
    fn plaintext_envelopes_carry_epoch_zero_and_signatures_bind_type_header_and_body() {
        let identity = SigningIdentity::generate().unwrap();
        let header = Header::new("member_publish", "desktop", "hub", 1);
        let mut envelope = Envelope::seal_plain(header, b"{\"name\":\"desktop\"}").unwrap();
        assert_eq!(envelope.header().unwrap().epoch, 0);
        assert!(envelope.verify_signature(&identity.public_key()).is_err());
        envelope.sign(&identity).unwrap();
        assert!(envelope.verify_signature(&identity.public_key()).is_ok());
        let opened = envelope.open(None).unwrap();
        assert!(!opened.encrypted);
        assert_eq!(opened.body, b"{\"name\":\"desktop\"}");

        let mut tampered_body = envelope.clone();
        tampered_body.body = crate::world::crypto::b64_encode(b"{\"name\":\"evil\"}");
        assert!(
            tampered_body
                .verify_signature(&identity.public_key())
                .is_err()
        );
        let other = SigningIdentity::generate().unwrap();
        assert!(envelope.verify_signature(&other.public_key()).is_err());
    }
}
