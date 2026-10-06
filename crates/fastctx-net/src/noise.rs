//! Sessions between two members: a Noise handshake, then two keys that seal
//! everything said on the session.
//!
//! The handshake is `Noise_IK_25519_ChaChaPoly_SHA256`. The member that
//! makes a call starts a session with the member it calls, knowing that
//! member's sealing key from the members table; the answer to the first
//! handshake message finishes it, and from then on both are authenticated
//! and what they say has forward secrecy. The initiator's own static key
//! travels inside the first message, and the responder checks it is the
//! sealing key the members table has for the member the envelope is from.
//!
//! What follows the handshake is not sealed by Noise's transport, whose
//! messages hold at most 64 KiB: the two keys Noise's Split derives seal
//! each message whole with ChaCha20-Poly1305, under the number the sender
//! signs the envelope with. Every member signs each number once (the
//! counter in `link::state`), so a key never sees a nonce twice, and the
//! envelope's header is the associated data, so a sealed body opens only in
//! the envelope it was made for. Nothing here has a lifetime: a session
//! lasts as long as both ends keep it in memory, and a nonce of 2^64 − 1,
//! which Noise reserves, would take a member signing a billion envelopes a
//! second for five centuries.

use std::fmt;

use chacha20poly1305::aead::{Aead as _, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit as _};
use fastctx_types::MachineName;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroizing;

use crate::seal::{SealKey, SealPublic};
use crate::trust::WorldId;

const PATTERN: &str = "Noise_IK_25519_ChaChaPoly_SHA256";

/// The largest handshake message: an ephemeral key, a sealed static key and
/// an empty sealed payload.
const MAX_HANDSHAKE: usize = 32 + (32 + 16) + 16;

/// Which session, chosen at random by the member that starts it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub [u8; 16]);

impl SessionId {
    pub fn generate() -> std::io::Result<Self> {
        crate::sign::random::<16>().map(Self)
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SessionId({self})")
    }
}

impl Serialize for SessionId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let invalid = || serde::de::Error::custom(format!("{text:?} is not a session id"));
        if text.len() != 32 {
            return Err(invalid());
        }
        let mut bytes = [0_u8; 16];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).map_err(|_| invalid())?;
        }
        Ok(Self(bytes))
    }
}

/// Binds a handshake to its World, its two ends by name, and its session,
/// so it completes only between the members that meant it.
fn prologue(world: &WorldId, initiator: &MachineName, responder: &MachineName, session: SessionId) -> Vec<u8> {
    let mut prologue = b"fastctx-world/3/noise\0".to_vec();
    for part in [world.to_string(), initiator.to_string(), responder.to_string()] {
        prologue.extend_from_slice(part.as_bytes());
        prologue.push(0);
    }
    prologue.extend_from_slice(&session.0);
    prologue
}

fn params() -> snow::params::NoiseParams {
    PATTERN.parse().expect("a valid Noise pattern")
}

/// Why a handshake did not complete.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Failed {
    #[error("the handshake message is malformed or not for this machine's key")]
    Malformed,
    #[error("the other end's key in the handshake is not the one the members table has for it")]
    WrongKey,
    #[error("the handshake failed: {0}")]
    Noise(String),
}

fn noise(error: snow::Error) -> Failed {
    Failed::Noise(error.to_string())
}

/// The first half of a session this member started: waiting for the
/// answer to its first handshake message.
pub struct Initiating {
    state: snow::HandshakeState,
    pub session: SessionId,
}

impl fmt::Debug for Initiating {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Initiating({})", self.session)
    }
}

/// Starts a session from `initiator` to `responder`, whose sealing key is
/// `remote`; returns it with the first handshake message.
pub fn initiate(
    world: &WorldId,
    initiator: &MachineName,
    local: &SealKey,
    responder: &MachineName,
    remote: &SealPublic,
) -> Result<(Initiating, Vec<u8>), Failed> {
    let session = SessionId::generate().map_err(|error| Failed::Noise(error.to_string()))?;
    let prologue = prologue(world, initiator, responder, session);
    let mut state = snow::Builder::new(params())
        .local_private_key(&local.seed()[..])
        .and_then(|builder| builder.remote_public_key(remote.as_bytes()))
        .and_then(|builder| builder.prologue(&prologue))
        .and_then(|builder| builder.build_initiator())
        .map_err(noise)?;
    let mut message = vec![0_u8; MAX_HANDSHAKE];
    let written = state.write_message(&[], &mut message).map_err(noise)?;
    message.truncate(written);
    Ok((Initiating { state, session }, message))
}

impl Initiating {
    /// Reads the responder's answer and returns the session's keys.
    pub fn finish(mut self, reply: &[u8]) -> Result<Keys, Failed> {
        let mut payload = vec![0_u8; MAX_HANDSHAKE];
        self.state.read_message(reply, &mut payload).map_err(|_| Failed::Malformed)?;
        if !self.state.is_handshake_finished() {
            return Err(Failed::Malformed);
        }
        let (to_responder, to_initiator) = self.state.dangerously_get_raw_split();
        Ok(Keys::new(self.session, to_responder, to_initiator))
    }
}

/// Answers the first handshake message of a session `initiator` started,
/// whose sealing key the members table says is `expected`; returns the
/// session's keys with the message that finishes the handshake.
pub fn respond(
    world: &WorldId,
    initiator: &MachineName,
    expected: &SealPublic,
    responder: &MachineName,
    local: &SealKey,
    session: SessionId,
    first: &[u8],
) -> Result<(Keys, Vec<u8>), Failed> {
    let prologue = prologue(world, initiator, responder, session);
    let mut state = snow::Builder::new(params())
        .local_private_key(&local.seed()[..])
        .and_then(|builder| builder.prologue(&prologue))
        .and_then(|builder| builder.build_responder())
        .map_err(noise)?;
    let mut payload = vec![0_u8; MAX_HANDSHAKE];
    state.read_message(first, &mut payload).map_err(|_| Failed::Malformed)?;
    if state.get_remote_static() != Some(expected.as_bytes().as_slice()) {
        return Err(Failed::WrongKey);
    }
    let mut message = vec![0_u8; MAX_HANDSHAKE];
    let written = state.write_message(&[], &mut message).map_err(noise)?;
    message.truncate(written);
    if !state.is_handshake_finished() {
        return Err(Failed::Malformed);
    }
    let (to_responder, to_initiator) = state.dangerously_get_raw_split();
    Ok((Keys::new(session, to_initiator, to_responder), message))
}

/// A finished session's keys, as one end holds them: one for what it
/// sends, one for what it receives.
pub struct Keys {
    pub session: SessionId,
    send: ChaCha20Poly1305,
    receive: ChaCha20Poly1305,
}

impl fmt::Debug for Keys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Keys({})", self.session)
    }
}

/// A sealed body that does not open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("it does not open with this session's key: it was made for another, or changed on the way")]
pub struct Unopenable;

impl Keys {
    fn new(session: SessionId, send: [u8; 32], receive: [u8; 32]) -> Self {
        let (send, receive) = (Zeroizing::new(send), Zeroizing::new(receive));
        Self {
            session,
            send: ChaCha20Poly1305::new(&(*send).into()),
            receive: ChaCha20Poly1305::new(&(*receive).into()),
        }
    }

    /// Seals `plaintext` as this end's message numbered `n`, bound to
    /// `header`.
    pub fn seal(&self, n: u64, header: &[u8], plaintext: &[u8]) -> Vec<u8> {
        self.send.encrypt(&nonce(n), Payload { msg: plaintext, aad: header }).expect("sealing never fails in memory")
    }

    /// Opens the other end's message numbered `n`, made for `header`.
    pub fn open(&self, n: u64, header: &[u8], sealed: &[u8]) -> Result<Vec<u8>, Unopenable> {
        self.receive.decrypt(&nonce(n), Payload { msg: sealed, aad: header }).map_err(|_| Unopenable)
    }
}

/// Noise's nonce for ChaChaPoly: 32 zero bits, then the number little-endian.
fn nonce(n: u64) -> chacha20poly1305::Nonce {
    let mut nonce = [0_u8; 12];
    nonce[4..].copy_from_slice(&n.to_le_bytes());
    nonce.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_seals_both_ways_and_only_between_its_ends() {
        let world = WorldId::try_from("w-0123456789abcdef".to_string()).unwrap();
        let (a, b) = (MachineName::try_from("a").unwrap(), MachineName::try_from("b").unwrap());
        let (a_key, b_key, stranger) =
            (SealKey::generate().unwrap(), SealKey::generate().unwrap(), SealKey::generate().unwrap());

        let (initiating, first) = initiate(&world, &a, &a_key, &b, &b_key.public()).unwrap();
        let session = initiating.session;
        assert_eq!(respond(&world, &a, &stranger.public(), &b, &b_key, session, &first).unwrap_err(), Failed::WrongKey);
        let other = MachineName::try_from("c").unwrap();
        assert!(respond(&world, &a, &a_key.public(), &other, &b_key, session, &first).is_err());
        let (b_keys, reply) = respond(&world, &a, &a_key.public(), &b, &b_key, session, &first).unwrap();
        let a_keys = initiating.finish(&reply).unwrap();

        let sealed = a_keys.seal(7, b"header", b"call");
        assert_eq!(b_keys.open(7, b"header", &sealed).unwrap(), b"call");
        assert_eq!(b_keys.open(8, b"header", &sealed), Err(Unopenable));
        assert_eq!(b_keys.open(7, b"other", &sealed), Err(Unopenable));
        assert_eq!(a_keys.open(7, b"header", &sealed), Err(Unopenable));
        let answer = b_keys.seal(7, b"header", b"answer");
        assert_eq!(a_keys.open(7, b"header", &answer).unwrap(), b"answer");
        assert_ne!(answer[..6], sealed[..6]);
    }
}
