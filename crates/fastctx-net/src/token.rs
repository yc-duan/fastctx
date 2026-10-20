//! One-time enrollment tokens (`world-network.md`, 登录与入网): what a
//! device that cannot type a password carries to log in, as a line of text
//! for a server or a QR code for a phone.
//!
//! A token holds only what a device needs before it has talked to anyone:
//! where the hub is and its certificate, which hub, and a secret. What the
//! token is for, which account, whether it registers the computer as a
//! machine, until when, waits at the hub under the public half of the key
//! the secret gives; the device proves it holds the secret by signing the
//! connection's TLS exporter with that key. A token is a secret until it is
//! used, since whoever holds it can log in.
//!
//! It is written as `FX5` followed by its bytes in Crockford's base32:
//! upper case letters and digits a person can read out, that a QR code
//! holds in its alphanumeric mode, and that pass through every shell
//! unquoted. The last two bytes check the rest, so a mistyped token is
//! told as such.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::ids::{CertPin, HubId};
use crate::sign::{Digest, SecretKey};

const PREFIX: &str = "FX5";
const KIND: u8 = 1;
/// A token that names relays as well, after its secret: one a release before
/// them does not read, so a token without relays stays of the first kind.
const KIND_RELAYS: u8 = 2;
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A token's secret, from which its one-time key comes.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenSecret(pub Zeroizing<[u8; 16]>);

impl std::fmt::Debug for TokenSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenSecret(..)")
    }
}

impl TokenSecret {
    pub fn generate() -> std::io::Result<Self> {
        Ok(Self(Zeroizing::new(crate::sign::random::<16>()?)))
    }

    /// The token's one-time key; the hub keeps its public half.
    pub fn key(&self) -> SecretKey {
        let mut seed = Zeroizing::new([0_u8; 32]);
        Hkdf::<Sha256>::new(None, &*self.0)
            .expand(b"fastctx-hub/4/token", &mut *seed)
            .expect("32 bytes is a valid HKDF output");
        SecretKey::from_seed(&seed)
    }
}

#[derive(Clone, Debug)]
pub struct Token {
    pub addresses: Vec<SocketAddr>,
    /// Machines of the account that carry a connection to the hub from their
    /// network, for a computer joining that reaches the hub no other way.
    pub relays: Vec<SocketAddr>,
    pub cert: CertPin,
    pub hub: HubId,
    pub secret: TokenSecret,
}

impl Token {
    pub fn to_text(&self) -> String {
        let mut bytes = vec![if self.relays.is_empty() { KIND } else { KIND_RELAYS }];
        addresses(&mut bytes, &self.addresses);
        bytes.extend_from_slice(&self.cert.0.0);
        bytes.extend_from_slice(&self.hub.bytes());
        bytes.extend_from_slice(&*self.secret.0);
        if !self.relays.is_empty() {
            addresses(&mut bytes, &self.relays);
        }
        let check = crate::sign::sha256(&bytes);
        bytes.extend_from_slice(&check[..2]);
        format!("{PREFIX}{}", encode(&bytes))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let text: String = text.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
        let upper = text.to_ascii_uppercase();
        let body = upper.strip_prefix(PREFIX).ok_or_else(|| {
            if upper.starts_with("FX4") {
                "this is an invitation of the network's third version; ask for an enrollment token instead".to_string()
            } else {
                format!("an enrollment token starts with {PREFIX}")
            }
        })?;
        let mistyped = || "the token is mistyped or cut short: check it against where it came from".to_string();
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
        if kind != KIND && kind != KIND_RELAYS {
            return Err("this token is of a kind this fastctx does not know; it may be newer".to_string());
        }
        let addresses = reader.addresses().ok_or_else(mistyped)?;
        let cert = CertPin(Digest(reader.array().ok_or_else(mistyped)?));
        let hub = HubId::from_bytes(reader.array().ok_or_else(mistyped)?);
        let secret = TokenSecret(Zeroizing::new(reader.array().ok_or_else(mistyped)?));
        let relays = if kind == KIND_RELAYS { reader.addresses().ok_or_else(mistyped)? } else { Vec::new() };
        if !reader.0.is_empty() {
            return Err(mistyped());
        }
        Ok(Token { addresses, relays, cert, hub, secret })
    }
}

fn addresses(bytes: &mut Vec<u8>, addresses: &[SocketAddr]) {
    bytes.push(u8::try_from(addresses.len()).expect("a token names at most 255 addresses"));
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
