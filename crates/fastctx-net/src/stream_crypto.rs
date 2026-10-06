//! Sealing a data connection's messages.
//!
//! A data connection runs between two members through the hub and any
//! relays, which pair its two ends and pass its messages on. The member
//! that asks for the connection makes a random key for it and hands it to
//! the other end inside a sealed call or answer, so only the two ends ever
//! hold it. Every message on the connection, bytes or a word, is sealed
//! whole with ChaCha20-Poly1305 under that key: the nonce is the message's
//! direction and its number in that direction, and the associated data is
//! the connection's id and the direction. A message dropped, repeated,
//! reordered or moved to the other direction does not open, so the ends
//! see either exactly what the other sent, in order, or a failure.

use std::fmt;

use chacha20poly1305::aead::{Aead as _, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit as _};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroizing;

use crate::wire::End;

/// A data connection's key.
#[derive(Clone)]
pub struct StreamKey(Zeroizing<[u8; 32]>);

impl StreamKey {
    pub fn generate() -> std::io::Result<Self> {
        Ok(Self(Zeroizing::new(crate::sign::random::<32>()?)))
    }
}

impl fmt::Debug for StreamKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StreamKey(..)")
    }
}

impl PartialEq for StreamKey {
    fn eq(&self, other: &Self) -> bool {
        *self.0 == *other.0
    }
}

impl Eq for StreamKey {}

impl Serialize for StreamKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        crate::sign::Secret(self.0.clone()).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StreamKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        crate::sign::Secret::deserialize(deserializer).map(|secret| Self(secret.0))
    }
}

/// What one message on a data connection is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Bytes(Vec<u8>),
    /// Something short one end says to the other, as JSON.
    Word(String),
}

const BYTES: u8 = 0;
const WORD: u8 = 1;

/// One end's sealing of a data connection: the next number in each
/// direction.
pub struct Sealing {
    cipher: ChaCha20Poly1305,
    stream: u64,
    end: End,
    sent: u64,
    received: u64,
}

/// A message that does not open.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Broken {
    #[error("a message on the data connection does not open: it was changed, dropped or reordered on the way")]
    Unopenable,
    #[error("a message on the data connection is malformed")]
    Malformed,
}

impl Sealing {
    /// `end`'s sealing of the connection `stream`.
    pub fn new(key: &StreamKey, stream: u64, end: End) -> Self {
        Self { cipher: ChaCha20Poly1305::new(&(*key.0).into()), stream, end, sent: 0, received: 0 }
    }

    pub fn seal(&mut self, piece: &Piece) -> Vec<u8> {
        let (tag, body) = match piece {
            Piece::Bytes(bytes) => (BYTES, bytes.as_slice()),
            Piece::Word(word) => (WORD, word.as_bytes()),
        };
        let mut plaintext = Vec::with_capacity(1 + body.len());
        plaintext.push(tag);
        plaintext.extend_from_slice(body);
        let direction = direction(self.end);
        let sealed = self
            .cipher
            .encrypt(&nonce(direction, self.sent), Payload { msg: &plaintext, aad: &aad(self.stream, direction) })
            .expect("sealing never fails in memory");
        self.sent += 1;
        sealed
    }

    pub fn open(&mut self, sealed: &[u8]) -> Result<Piece, Broken> {
        let direction = direction(other(self.end));
        let plaintext = self
            .cipher
            .decrypt(&nonce(direction, self.received), Payload { msg: sealed, aad: &aad(self.stream, direction) })
            .map_err(|_| Broken::Unopenable)?;
        self.received += 1;
        match plaintext.split_first() {
            Some((&BYTES, body)) => Ok(Piece::Bytes(body.to_vec())),
            Some((&WORD, body)) => String::from_utf8(body.to_vec()).map(Piece::Word).map_err(|_| Broken::Malformed),
            _ => Err(Broken::Malformed),
        }
    }
}

/// The bytes a sealed message adds to what it carries.
pub const OVERHEAD: usize = 1 + 16;

fn direction(end: End) -> u8 {
    match end {
        End::Source => 0,
        End::Sink => 1,
    }
}

fn other(end: End) -> End {
    match end {
        End::Source => End::Sink,
        End::Sink => End::Source,
    }
}

fn nonce(direction: u8, n: u64) -> chacha20poly1305::Nonce {
    let mut nonce = [0_u8; 12];
    nonce[0] = direction;
    nonce[4..].copy_from_slice(&n.to_le_bytes());
    nonce.into()
}

fn aad(stream: u64, direction: u8) -> [u8; 9] {
    let mut aad = [0_u8; 9];
    aad[..8].copy_from_slice(&stream.to_le_bytes());
    aad[8] = direction;
    aad
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_open_once_in_order_in_their_direction() {
        let key = StreamKey::generate().unwrap();
        let (mut source, mut sink) = (Sealing::new(&key, 9, End::Source), Sealing::new(&key, 9, End::Sink));
        let first = source.seal(&Piece::Bytes(vec![1, 2, 3]));
        let second = source.seal(&Piece::Word("\"done\"".to_string()));
        assert_eq!(sink.open(&second), Err(Broken::Unopenable));
        let mut sink = Sealing::new(&key, 9, End::Sink);
        assert_eq!(sink.open(&first).unwrap(), Piece::Bytes(vec![1, 2, 3]));
        assert_eq!(sink.open(&first), Err(Broken::Unopenable));
        let mut sink = Sealing::new(&key, 9, End::Sink);
        sink.open(&first).unwrap();
        assert_eq!(sink.open(&second).unwrap(), Piece::Word("\"done\"".to_string()));
        let back = sink.seal(&Piece::Bytes(vec![4]));
        assert_eq!(source.open(&back).unwrap(), Piece::Bytes(vec![4]));
        let mut elsewhere = Sealing::new(&key, 10, End::Sink);
        assert_eq!(elsewhere.open(&first), Err(Broken::Unopenable));
    }
}
