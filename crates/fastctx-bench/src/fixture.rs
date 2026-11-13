//! Fixed inputs: printable ASCII from a fixed seed (zstd's first level
//! compresses it about 1.3–1.6 times, as the design asks, never all zeros
//! or a repeated string), and timeline records made of it.

use fastctx_content::v2::{EVENT, Kind as _, KindRecord, Timeline};
use fastctx_content::{Record, State};

/// A fixed-seed generator (xorshift64*): the same seed, the same bytes.
pub(crate) struct Seeded(u64);

impl Seeded {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub(crate) fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A number in `low..=high`.
    pub(crate) fn between(&mut self, low: usize, high: usize) -> usize {
        low + (self.next() % (high - low + 1) as u64) as usize
    }

    /// `length` printable ASCII bytes, words of a few letters apart.
    pub(crate) fn text(&mut self, length: usize) -> String {
        let mut text = String::with_capacity(length);
        while text.len() < length {
            let word = self.between(2, 9);
            for _ in 0..word.min(length - text.len()) {
                text.push(char::from(b'a' + (self.next() % 26) as u8));
            }
            if text.len() < length {
                text.push(if self.next().is_multiple_of(11) { '\n' } else { ' ' });
            }
        }
        text
    }

    /// `length` random bytes.
    pub(crate) fn bytes(&mut self, length: usize) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(length + 8);
        while bytes.len() < length {
            bytes.extend_from_slice(&self.next().to_le_bytes());
        }
        bytes.truncate(length);
        bytes
    }
}

/// The `number`th event of a timeline, its body `bytes` long or about it.
pub(crate) fn event(number: u64, bytes: usize, seeded: &mut Seeded) -> KindRecord {
    let text = seeded.text(bytes.saturating_sub(48));
    let mut body = vec![EVENT];
    body.extend_from_slice(&serde_json::to_vec(&serde_json::json!({ "type": "assistant", "n": number, "text": text })).expect("an event"));
    let key = number.to_be_bytes().to_vec();
    KindRecord { kind: Timeline::KIND, schema: Timeline::SCHEMA, record: Record { item: key.clone(), order: key, state: State::Is { body, objects: Vec::new() } } }
}
