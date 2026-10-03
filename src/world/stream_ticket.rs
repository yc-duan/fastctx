//! A stream author names its two endpoints and the permission the hub must check.
use super::crypto::{b64_decode, b64_encode};
use super::identity::{self, SigningIdentity};
use super::wire::StreamRole;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Ticket {
    pub(crate) world: String,
    pub(crate) stream: u64,
    pub(crate) source: String,
    pub(crate) sink: String,
    pub(crate) principal: String,
    pub(crate) target: String,
    pub(crate) verb: String,
    pub(crate) sig: String,
}
impl Ticket {
    pub(crate) fn bytes(&self) -> Result<Vec<u8>, String> {
        super::messages::encode(&(
            &self.world,
            self.stream,
            &self.source,
            &self.sink,
            &self.principal,
            &self.target,
            &self.verb,
        ))
    }
    pub(crate) fn sign(&mut self, identity: &SigningIdentity) -> Result<(), String> {
        self.sig = b64_encode(&identity.sign("stream_ticket", &self.bytes()?));
        Ok(())
    }
    pub(crate) fn verify(&self, key: &[u8; 32]) -> Result<(), String> {
        identity::verify(
            key,
            "stream_ticket",
            &self.bytes()?,
            &b64_decode(&self.sig)?,
        )
    }
    pub(crate) fn endpoint(&self, role: StreamRole) -> &str {
        match role {
            StreamRole::Source => &self.source,
            StreamRole::Sink => &self.sink,
        }
    }
}
