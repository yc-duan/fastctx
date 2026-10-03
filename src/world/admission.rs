//! Historical admission proofs. Revocation affects the speaker, never its admitted descendants.

use super::crypto::{b64_array, b64_decode, b64_encode};
use super::identity::{self, Identity, SigningIdentity};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Endorsement {
    pub(crate) world: String,
    pub(crate) inviter: String,
    pub(crate) invite_pub: String,
    pub(crate) expires: String,
    pub(crate) grant_floor: u64,
    pub(crate) name: Option<String>,
    pub(crate) sig: String,
}

impl Endorsement {
    fn bytes(&self) -> Result<Vec<u8>, String> {
        super::messages::encode(&(
            &self.world,
            &self.inviter,
            &self.invite_pub,
            &self.expires,
            self.grant_floor,
            &self.name,
        ))
    }
    pub(crate) fn sign(&mut self, identity: &Identity) -> Result<(), String> {
        self.sig = b64_encode(&identity.sign("invite_endorsement", &self.bytes()?));
        Ok(())
    }
    pub(crate) fn verify(&self, key: &[u8; 32]) -> Result<(), String> {
        identity::verify(
            key,
            "invite_endorsement",
            &self.bytes()?,
            &b64_decode(&self.sig)?,
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Credential {
    Founder,
    Invited {
        endorsement: Endorsement,
        proof: String,
    },
}

impl Credential {
    pub(crate) fn invited(
        endorsement: Endorsement,
        seed: &[u8; 32],
        name: &str,
        public: &str,
    ) -> Result<Self, String> {
        let bytes = super::messages::encode(&(&endorsement, name, public))?;
        Ok(Self::Invited {
            endorsement,
            proof: b64_encode(&SigningIdentity::from_seed(seed).sign("admission", &bytes)),
        })
    }
    pub(crate) fn verify(
        &self,
        name: &str,
        public: &str,
        inviter_key: &[u8; 32],
    ) -> Result<(), String> {
        let Self::Invited { endorsement, proof } = self else {
            return Err("expected an invited admission".into());
        };
        endorsement.verify(inviter_key)?;
        if endorsement
            .name
            .as_deref()
            .is_some_and(|expected| expected != name)
        {
            return Err("invite names a different member".into());
        }
        identity::verify(
            &b64_array(&endorsement.invite_pub, "invitation public key")?,
            "admission",
            &super::messages::encode(&(endorsement, name, public))?,
            &b64_decode(proof)?,
        )
    }
}
