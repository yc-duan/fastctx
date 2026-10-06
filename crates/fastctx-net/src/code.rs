//! The codes a person carries from one machine to another to join a World:
//! the hub's founding code for the first machine, and an invitation for
//! every machine and device after it (`world-network.md`, 邀请码).
//!
//! A code holds only what a machine needs before it has talked to anyone:
//! where its upstream is and that upstream's certificate, which World, the
//! founder's key, and a secret. An invitation itself, what it admits and
//! until when, waits at the hub; the joining machine proves it holds the
//! secret to get it. A code is a secret until it is used, since whoever
//! holds it can join.
//!
//! It is written as `FX4` followed by its bytes in Crockford's base32: upper
//! case letters and digits a person can read out, that a QR code holds in
//! its alphanumeric mode, and that pass through every shell unquoted. The
//! last two bytes check the rest, so a mistyped code is told as such.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::sign::{Digest, PublicKey, Secret, SecretKey};
use crate::trust::{CertPin, WorldId};

const PREFIX: &str = "FX4";
const FOUNDING: u8 = 1;
const INVITATION: u8 = 2;
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The secret of an invitation, from which its one-time key comes.
#[derive(Clone, PartialEq, Eq)]
pub struct InviteSecret(pub Zeroizing<[u8; 16]>);

impl std::fmt::Debug for InviteSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InviteSecret(..)")
    }
}

impl InviteSecret {
    pub fn generate() -> std::io::Result<Self> {
        Ok(Self(Zeroizing::new(crate::sign::random::<16>()?)))
    }

    /// The invitation's one-time key.
    pub fn key(&self) -> SecretKey {
        let mut seed = Zeroizing::new([0_u8; 32]);
        Hkdf::<Sha256>::new(None, &*self.0)
            .expand(b"fastctx-world/3/invite", &mut *seed)
            .expect("32 bytes is a valid HKDF output");
        SecretKey::from_seed(&seed)
    }
}

#[derive(Clone, Debug)]
pub enum Code {
    /// Printed by a new hub: the first machine to join with it founds the
    /// World.
    Founding { hub: Vec<SocketAddr>, cert: CertPin, world: WorldId, secret: Secret },
    /// Made by a member: the machine that joins with it fetches the
    /// invitation from the hub with the key `secret` gives.
    Invitation { hub: Vec<SocketAddr>, cert: CertPin, world: WorldId, founder: PublicKey, secret: InviteSecret },
}

impl Code {
    pub fn to_text(&self) -> String {
        let mut bytes = Vec::new();
        match self {
            Code::Founding { hub, cert, world, secret } => {
                bytes.push(FOUNDING);
                addresses(&mut bytes, hub);
                bytes.extend_from_slice(&cert.0.0);
                bytes.extend_from_slice(&world.bytes());
                bytes.extend_from_slice(&*secret.0);
            }
            Code::Invitation { hub, cert, world, founder, secret } => {
                bytes.push(INVITATION);
                addresses(&mut bytes, hub);
                bytes.extend_from_slice(&cert.0.0);
                bytes.extend_from_slice(&world.bytes());
                bytes.extend_from_slice(founder.as_bytes());
                bytes.extend_from_slice(&*secret.0);
            }
        }
        let check = crate::sign::sha256(&bytes);
        bytes.extend_from_slice(&check[..2]);
        format!("{PREFIX}{}", encode(&bytes))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let text: String = text.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
        let upper = text.to_ascii_uppercase();
        let body = upper.strip_prefix(PREFIX).ok_or_else(|| format!("a World code starts with {PREFIX}"))?;
        let mistyped = || "the code is mistyped or cut short: check it against where it came from".to_string();
        let bytes = decode(body).ok_or_else(mistyped)?;
        if bytes.len() < 3 {
            return Err(mistyped());
        }
        let (bytes, check) = bytes.split_at(bytes.len() - 2);
        if crate::sign::sha256(bytes)[..2] != *check {
            return Err(mistyped());
        }
        let mut reader = Reader(bytes);
        let kind = reader.take(1).ok_or_else(mistyped)?[0];
        let hub = reader.addresses().ok_or_else(mistyped)?;
        let cert = CertPin(Digest(reader.array().ok_or_else(mistyped)?));
        let world = WorldId::from_bytes(reader.array().ok_or_else(mistyped)?);
        let code = match kind {
            FOUNDING => Code::Founding {
                hub,
                cert,
                world,
                secret: Secret(Zeroizing::new(reader.array().ok_or_else(mistyped)?)),
            },
            INVITATION => {
                let founder = PublicKey::from_bytes(reader.array().ok_or_else(mistyped)?);
                let secret = InviteSecret(Zeroizing::new(reader.array().ok_or_else(mistyped)?));
                Code::Invitation { hub, cert, world, founder, secret }
            }
            _ => return Err("this code is of a kind this fastctx does not know; it may be newer".to_string()),
        };
        if !reader.0.is_empty() {
            return Err(mistyped());
        }
        Ok(code)
    }

    pub fn hub(&self) -> &[SocketAddr] {
        match self {
            Code::Founding { hub, .. } | Code::Invitation { hub, .. } => hub,
        }
    }

    pub fn cert(&self) -> CertPin {
        match self {
            Code::Founding { cert, .. } | Code::Invitation { cert, .. } => *cert,
        }
    }

    pub fn world(&self) -> &WorldId {
        match self {
            Code::Founding { world, .. } | Code::Invitation { world, .. } => world,
        }
    }
}

fn addresses(bytes: &mut Vec<u8>, addresses: &[SocketAddr]) {
    bytes.push(u8::try_from(addresses.len()).expect("a code names at most 255 addresses"));
    for address in addresses {
        match address.ip() {
            IpAddr::V4(ip) => {
                bytes.push(4);
                bytes.extend_from_slice(&ip.octets());
            }
            IpAddr::V6(ip) => {
                bytes.push(6);
                bytes.extend_from_slice(&ip.octets());
            }
        }
        bytes.extend_from_slice(&address.port().to_be_bytes());
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        if self.0.len() < count {
            return None;
        }
        let (taken, rest) = self.0.split_at(count);
        self.0 = rest;
        Some(taken)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N).map(|bytes| bytes.try_into().expect("N bytes"))
    }

    fn addresses(&mut self) -> Option<Vec<SocketAddr>> {
        let count = self.take(1)?[0];
        let mut addresses = Vec::new();
        for _ in 0..count {
            let ip = match self.take(1)?[0] {
                4 => IpAddr::V4(Ipv4Addr::from(self.array::<4>()?)),
                6 => IpAddr::V6(Ipv6Addr::from(self.array::<16>()?)),
                _ => return None,
            };
            let port = u16::from_be_bytes(self.array()?);
            addresses.push(SocketAddr::new(ip, port));
        }
        Some(addresses)
    }
}

/// Crockford's base32, five bits a letter, the last letter's spare bits zero.
fn encode(bytes: &[u8]) -> String {
    let mut text = String::new();
    let (mut buffer, mut bits) = (0_u32, 0);
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            text.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        text.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    text
}

/// Reads Crockford's base32, taking O for 0 and I and L for 1 as a person
/// may write them.
fn decode(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let (mut buffer, mut bits) = (0_u32, 0);
    for c in text.bytes() {
        let c = match c {
            b'O' => b'0',
            b'I' | b'L' => b'1',
            other => other,
        };
        let value = ALPHABET.iter().position(|letter| *letter == c)? as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
        }
    }
    // What is left over is padding, and must be zero.
    if bits >= 5 || buffer & ((1 << bits) - 1) != 0 {
        return None;
    }
    Some(bytes)
}
