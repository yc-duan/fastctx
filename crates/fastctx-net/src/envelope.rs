//! What one member says to another: an envelope the hub and relays can
//! route, around a body only the other member can open.
//!
//! The envelope names the World, who it is from and for, the number its
//! sender signs it under, what kind of thing it carries and, for what
//! travels on a session, which session; all of it is signed by the sender,
//! so nobody on the way can change it or make one up. The body is sealed:
//! with the session's keys for calls and answers (`crate::noise`), with
//! HPKE to the receiver's sealing key for a letter, which may wait for a
//! receiver that is away (`crate::seal`). The header is the associated data
//! of either, so a body opens only in the envelope it was sealed for.

use fastctx_types::MachineName;
use serde::{Deserialize, Serialize};

use crate::noise::SessionId;
use crate::sign::{Bytes, Domain, PublicKey, SecretKey, Signed};
use crate::trust::WorldId;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub world: WorldId,
    pub from: MachineName,
    pub to: MachineName,
    /// Counts up across everything the sender ever signs and is never
    /// reused: the receiver refuses a call it took before, and a session's
    /// key never seals two bodies under one number.
    pub n: u64,
    pub kind: Kind,
    /// The session a call, an answer or a handshake belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    /// For what answers a call, that call's number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub re: Option<u64>,
    pub sealed: Bytes,
}

/// What an envelope carries; the hub and relays route by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A call, sealed with the caller's session.
    Call,
    /// The answer to a call, sealed with the same session.
    Answer,
    /// A letter, sealed to the receiver's key.
    Letter,
    /// The first message of a session's handshake; it travels as a call.
    Handshake,
    /// The message that finishes a handshake; it travels as its answer.
    HandshakeReply,
    /// The receiver does not have the session a call came on, so it did not
    /// open the call, and nothing of it ran; it travels as the answer.
    UnknownSession,
    /// The receiver refused a call or a handshake before opening it: it was
    /// not a member's, or taken before. The body is the reason, in plain
    /// words that name nothing of the call; it travels as the answer.
    Refused,
}

impl Kind {
    /// Whether it travels as a call: on to the receiver, which answers once.
    pub fn is_call(self) -> bool {
        matches!(self, Kind::Call | Kind::Handshake)
    }

    /// Whether it travels as an answer to a call.
    pub fn is_answer(self) -> bool {
        matches!(self, Kind::Answer | Kind::HandshakeReply | Kind::UnknownSession | Kind::Refused)
    }
}

/// The envelope without its body: what the body is bound to.
#[derive(Serialize)]
struct Header<'a> {
    world: &'a WorldId,
    from: &'a MachineName,
    to: &'a MachineName,
    n: u64,
    kind: Kind,
    session: &'a Option<SessionId>,
    re: &'a Option<u64>,
}

impl Envelope {
    /// The bytes a body sealed for this envelope is bound to.
    pub fn header(&self) -> Vec<u8> {
        header(&self.world, &self.from, &self.to, self.n, self.kind, &self.session, &self.re)
    }

    pub fn sign(&self, key: &SecretKey) -> Signed {
        Signed::new(key, Domain::Envelope, self)
    }
}

/// The header of an envelope still to be made, for sealing its body.
pub fn header(
    world: &WorldId,
    from: &MachineName,
    to: &MachineName,
    n: u64,
    kind: Kind,
    session: &Option<SessionId>,
    re: &Option<u64>,
) -> Vec<u8> {
    serde_json::to_vec(&Header { world, from, to, n, kind, session, re }).expect("headers serialize")
}

/// The envelope in `signed`, as it claims to be, before its sender is
/// known to have signed it.
pub fn claim(signed: &Signed) -> Result<Envelope, String> {
    signed.claim().map_err(|error| format!("an unreadable envelope: {error}"))
}

/// The envelope in `signed`, if `key` signed it.
pub fn open(signed: &Signed, key: &PublicKey) -> Result<Envelope, String> {
    signed.open(key, Domain::Envelope).map_err(|error| format!("the envelope: {error}"))
}
