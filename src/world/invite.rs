//! A one-time signing seed delivered to the joining device, never to the hub.
use super::admission::{Credential, Endorsement};
use super::crypto::{self, b64url_decode, b64url_encode};
use super::identity::{Fingerprint, Identity, SigningIdentity};
use serde::{Deserialize, Serialize};

pub(crate) const INVITE_PREFIX: &str = "fxw2.";
#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Invite {
    pub(crate) hub: Vec<String>,
    pub(crate) hub_key: String,
    pub(crate) founder: String,
    pub(crate) inviter_key: String,
    seed: [u8; 32],
    pub(crate) endorsement: Endorsement,
}
impl std::fmt::Debug for Invite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Invite")
            .field("endorsement", &self.endorsement)
            .finish_non_exhaustive()
    }
}
impl Invite {
    pub(crate) fn new(
        config: &super::WorldConfig,
        identity: &Identity,
        name: Option<String>,
        ttl: time::Duration,
        grant_floor: u64,
        hub: Vec<String>,
    ) -> Result<Self, String> {
        if hub.is_empty() {
            return Err("An invite needs a hub address".into());
        }
        let seed = crypto::random_bytes()?;
        let mut endorsement = Endorsement {
            world: config.world_id.clone(),
            inviter: config.name.clone(),
            invite_pub: crypto::b64_encode(&SigningIdentity::from_seed(&seed).public_key()),
            expires: super::format_rfc3339(time::OffsetDateTime::now_utc() + ttl),
            grant_floor,
            name,
            sig: String::new(),
        };
        endorsement.sign(identity)?;
        Ok(Self {
            hub,
            hub_key: config.hub_key.clone(),
            founder: config.founder.clone(),
            inviter_key: crypto::b64_encode(&identity.public_key()),
            seed,
            endorsement,
        })
    }
    pub(crate) fn encode(&self) -> String {
        format!(
            "{INVITE_PREFIX}{}",
            b64url_encode(&serde_json::to_vec(self).expect("invite serializes"))
        )
    }
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let payload = text
            .trim()
            .strip_prefix(INVITE_PREFIX)
            .ok_or("Expected a protocol v2 invite")?;
        let invite: Self = serde_json::from_slice(&b64url_decode(payload)?)
            .map_err(|e| format!("Invalid invite: {e}"))?;
        Fingerprint::parse(&invite.hub_key)?;
        crypto::b64_array::<32>(&invite.founder, "founder")?;
        invite
            .endorsement
            .verify(&crypto::b64_array(&invite.inviter_key, "inviter")?)?;
        if crypto::b64_encode(&SigningIdentity::from_seed(&invite.seed).public_key())
            != invite.endorsement.invite_pub
        {
            return Err("Invitation seed does not match its endorsement".into());
        }
        Ok(invite)
    }
    pub(crate) fn is_expired_at(&self, now: time::OffsetDateTime) -> bool {
        super::parse_rfc3339(&self.endorsement.expires).map_or(true, |expiry| expiry <= now)
    }
    pub(crate) fn credential(&self, name: &str, public: &str) -> Result<Credential, String> {
        Credential::invited(self.endorsement.clone(), &self.seed, name, public)
    }
    pub(crate) fn admission_of_token(token: &[u8]) -> String {
        hex::encode(crypto::sha256(token))
    }
}

impl std::ops::Deref for Invite {
    type Target = Endorsement;
    fn deref(&self) -> &Endorsement {
        &self.endorsement
    }
}
