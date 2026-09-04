//! Invites: the pasteable string that tells a new machine where the hub is, which hub to
//! trust, and how to prove its admission, and the derived values the hub is allowed to hold.
//!
//! The hub never learns the invite secret. It stores `code_id` (to find the invite),
//! `admission` (to check the one-time token), and the World keys wrapped under a key only the
//! secret can derive. Knowing all three still cannot unwrap the keys or forge a member.

use super::crypto::{self, Key32, b64url_decode, b64url_encode, sha256};
use super::identity::Fingerprint;
use super::keys::KeyRing;
use super::{format_rfc3339, parse_rfc3339};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Prefix of every invite string; the digit is the invite format version.
pub(crate) const INVITE_PREFIX: &str = "fxw1.";
/// Default lifetime of an invite.
pub(crate) const DEFAULT_INVITE_TTL: time::Duration = time::Duration::hours(24);
const INVITE_FORMAT_VERSION: u32 = 1;

/// A decoded invite.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub(crate) struct Invite {
    /// Hub addresses (`host:port`), tried in order.
    #[zeroize(skip)]
    pub(crate) hub: Vec<String>,
    /// The hub identity the new machine must see in `challenge`.
    #[zeroize(skip)]
    pub(crate) hub_key: Fingerprint,
    /// 32 random bytes; everything else is derived from them.
    secret: Key32,
    /// Suggested member name.
    #[zeroize(skip)]
    pub(crate) name: Option<String>,
    /// Expiry, RFC 3339 UTC.
    #[zeroize(skip)]
    pub(crate) exp: String,
}

#[derive(Deserialize, Serialize)]
struct InviteWire {
    v: u32,
    hub: Vec<String>,
    hub_key: String,
    secret: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    exp: String,
}

impl Invite {
    /// Creates a fresh invite expiring `ttl` from now.
    pub(crate) fn new(
        hub: Vec<String>,
        hub_key: Fingerprint,
        name: Option<String>,
        ttl: time::Duration,
    ) -> Result<Self, String> {
        if hub.is_empty() {
            return Err("An invite needs at least one hub address.".to_string());
        }
        Ok(Self {
            hub,
            hub_key,
            secret: crypto::random_bytes::<32>()?,
            name,
            exp: format_rfc3339(time::OffsetDateTime::now_utc() + ttl),
        })
    }

    /// The pasteable form: `fxw1.` plus base64url JSON.
    pub(crate) fn encode(&self) -> String {
        let wire = InviteWire {
            v: INVITE_FORMAT_VERSION,
            hub: self.hub.clone(),
            hub_key: self.hub_key.to_string(),
            secret: b64url_encode(&self.secret),
            name: self.name.clone(),
            exp: self.exp.clone(),
        };
        let json = serde_json::to_vec(&wire).expect("an invite serializes");
        format!("{INVITE_PREFIX}{}", b64url_encode(&json))
    }

    /// Parses a pasted invite string (surrounding whitespace tolerated).
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let Some(payload) = text.strip_prefix(INVITE_PREFIX) else {
            return Err(format!(
                "That is not a FastCtx World invite: it must start with \"{INVITE_PREFIX}\"."
            ));
        };
        let json = b64url_decode(payload)
            .map_err(|_| "That invite is not intact; paste the whole string.".to_string())?;
        let wire: InviteWire = serde_json::from_slice(&json)
            .map_err(|_| "That invite is not intact; paste the whole string.".to_string())?;
        if wire.v != INVITE_FORMAT_VERSION {
            return Err(format!(
                "That invite uses format {} but this fastctx understands format {INVITE_FORMAT_VERSION}. Upgrade the older side.",
                wire.v
            ));
        }
        if wire.hub.is_empty() {
            return Err("That invite names no hub address.".to_string());
        }
        parse_rfc3339(&wire.exp)?;
        Ok(Self {
            hub: wire.hub,
            hub_key: Fingerprint::parse(&wire.hub_key)?,
            secret: crypto::b64_array::<32>(
                &wire.secret.replace('-', "+").replace('_', "/"),
                "invite secret",
            )?,
            name: wire.name,
            exp: wire.exp,
        })
    }

    pub(crate) fn is_expired_at(&self, now: time::OffsetDateTime) -> bool {
        parse_rfc3339(&self.exp)
            .map(|exp| exp <= now)
            .unwrap_or(true)
    }

    /// `sha256("invite" ‖ secret)`, hex: how the hub files the invite.
    pub(crate) fn code_id(&self) -> String {
        hex::encode(crypto::sha256_parts(&[b"invite", &self.secret]))
    }

    /// `HMAC(secret, "admission")`: the one-time bearer presented at enrollment.
    pub(crate) fn admission_token(&self) -> Key32 {
        crypto::hmac_sha256(&self.secret, b"admission")
    }

    /// `sha256(admission_token)`, hex: what the hub stores to check the token.
    pub(crate) fn admission(&self) -> String {
        hex::encode(sha256(&self.admission_token()))
    }

    /// Hex of the hash the hub stores for a presented admission token.
    pub(crate) fn admission_of_token(token: &[u8]) -> String {
        hex::encode(sha256(token))
    }

    /// Wraps every World key epoch under `HKDF(secret, "wrap")`, bound to the code id.
    pub(crate) fn wrap_keys(&self, ring: &KeyRing) -> Result<String, String> {
        super::keys::seal_blob(
            &self.wrap_key(),
            self.code_id().as_bytes(),
            &ring.to_plain_json(),
        )
    }

    /// Unwraps the keys the hub handed back at enrollment.
    pub(crate) fn unwrap_keys(&self, wrapped: &str) -> Result<KeyRing, String> {
        let plain = super::keys::open_blob(&self.wrap_key(), self.code_id().as_bytes(), wrapped)
            .map_err(|_| "The World keys in the hub's answer do not unwrap with this invite; the invite and the hub disagree.".to_string())?;
        KeyRing::from_plain_json(&plain)
    }

    fn wrap_key(&self) -> Key32 {
        crypto::hkdf_derive(&self.secret, b"wrap")
    }
}

impl std::fmt::Debug for Invite {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Invite")
            .field("hub", &self.hub)
            .field("hub_key", &self.hub_key)
            .field("name", &self.name)
            .field("exp", &self.exp)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_INVITE_TTL, Invite};
    use crate::world::identity::Fingerprint;
    use crate::world::keys::KeyRing;

    #[test]
    fn an_invite_round_trips_and_only_its_secret_unwraps_the_keys() {
        let hub_key = Fingerprint::of(&[3_u8; 32]);
        let invite = Invite::new(
            vec!["hub.example:443".to_string(), "203.0.113.5:443".to_string()],
            hub_key,
            Some("laptop".to_string()),
            DEFAULT_INVITE_TTL,
        )
        .unwrap();
        let encoded = invite.encode();
        assert!(encoded.starts_with("fxw1."));
        let parsed = Invite::parse(&format!("  {encoded}\n")).unwrap();
        assert_eq!(parsed.hub, invite.hub);
        assert_eq!(parsed.hub_key, hub_key);
        assert_eq!(parsed.name.as_deref(), Some("laptop"));
        assert_eq!(parsed.code_id(), invite.code_id());
        assert_eq!(parsed.admission(), invite.admission());
        assert_eq!(
            Invite::admission_of_token(&parsed.admission_token()),
            invite.admission()
        );
        assert!(!parsed.is_expired_at(time::OffsetDateTime::now_utc()));

        let ring = KeyRing::new_initial().unwrap();
        let wrapped = invite.wrap_keys(&ring).unwrap();
        assert_eq!(parsed.unwrap_keys(&wrapped).unwrap().epochs(), vec![1]);
        let other = Invite::new(
            vec!["hub.example:443".to_string()],
            hub_key,
            None,
            DEFAULT_INVITE_TTL,
        )
        .unwrap();
        assert!(other.unwrap_keys(&wrapped).is_err());

        assert!(Invite::parse("fxw2.abc").is_err());
        assert!(Invite::parse(&encoded[..encoded.len() - 3]).is_err());
    }
}
