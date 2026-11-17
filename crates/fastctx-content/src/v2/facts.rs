//! The hub's signed facts about a stream's succession (§2.2, §8), as
//! encoded: what the hub signs and every holder checks before it trusts
//! one. An epoch descriptor names the device that orders a run of the
//! stream, where the run starts and the chain value it follows; a gap fact
//! says a predecessor's tail will never come, so holders skip it on the
//! hub's word.

/// What an epoch's records come from (§2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EpochKind {
    Source,
    Observed,
}

impl EpochKind {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Observed => "observed",
        }
    }

    fn named(name: &str) -> Result<Self, String> {
        match name {
            "source" => Ok(Self::Source),
            "observed" => Ok(Self::Observed),
            _ => Err("an epoch of an unknown kind".into()),
        }
    }
}

/// An epoch descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpochFact {
    pub scope: String,
    pub stream: String,
    pub epoch: u64,
    pub device: [u8; 32],
    pub key: [u8; 32],
    pub start: u64,
    pub kind: EpochKind,
    pub predecessor: Option<u64>,
    /// The chain value the epoch's first frame follows.
    pub follows: [u8; 32],
}

/// A gap fact: `from` to `to` of `epoch` will never come; what follows
/// chains from `follows`, and what came before ended at `start`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GapFact {
    pub scope: String,
    pub stream: String,
    pub epoch: u64,
    pub from: u64,
    pub to: u64,
    pub start: [u8; 32],
    pub follows: [u8; 32],
    pub why: String,
}

const EPOCH: &[u8] = b"fastctx-epoch/1";
const GAP: &[u8] = b"fastctx-gap/1";

fn put(bytes: &mut Vec<u8>, field: &[u8]) {
    bytes.extend_from_slice(&(field.len() as u32).to_le_bytes());
    bytes.extend_from_slice(field);
}

/// Reads a fact's fields in order.
struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, length: usize) -> Result<&[u8], String> {
        if self.0.len() < length {
            return Err("a fact is cut short".into());
        }
        let (taken, rest) = self.0.split_at(length);
        self.0 = rest;
        Ok(taken)
    }

    fn number(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }

    fn array(&mut self) -> Result<[u8; 32], String> {
        Ok(self.take(32)?.try_into().expect("32 bytes"))
    }

    fn text(&mut self) -> Result<String, String> {
        let length = u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")) as usize;
        String::from_utf8(self.take(length)?.to_vec()).map_err(|_| "a fact's text is not UTF-8".to_string())
    }

    fn end(&self) -> Result<(), String> {
        if self.0.is_empty() { Ok(()) } else { Err("a fact has bytes past its end".into()) }
    }
}

impl EpochFact {
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = EPOCH.to_vec();
        put(&mut bytes, self.scope.as_bytes());
        put(&mut bytes, self.stream.as_bytes());
        bytes.extend_from_slice(&self.epoch.to_le_bytes());
        bytes.extend_from_slice(&self.device);
        bytes.extend_from_slice(&self.key);
        bytes.extend_from_slice(&self.start.to_le_bytes());
        put(&mut bytes, self.kind.name().as_bytes());
        bytes.extend_from_slice(&self.predecessor.unwrap_or(u64::MAX).to_le_bytes());
        bytes.extend_from_slice(&self.follows);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let rest = bytes.strip_prefix(EPOCH).ok_or("not an epoch descriptor")?;
        let mut reader = Reader(rest);
        let fact = Self {
            scope: reader.text()?,
            stream: reader.text()?,
            epoch: reader.number()?,
            device: reader.array()?,
            key: reader.array()?,
            start: reader.number()?,
            kind: EpochKind::named(&reader.text()?)?,
            predecessor: Some(reader.number()?).filter(|epoch| *epoch != u64::MAX),
            follows: reader.array()?,
        };
        reader.end()?;
        Ok(fact)
    }
}

impl GapFact {
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = GAP.to_vec();
        put(&mut bytes, self.scope.as_bytes());
        put(&mut bytes, self.stream.as_bytes());
        bytes.extend_from_slice(&self.epoch.to_le_bytes());
        bytes.extend_from_slice(&self.from.to_le_bytes());
        bytes.extend_from_slice(&self.to.to_le_bytes());
        bytes.extend_from_slice(&self.start);
        bytes.extend_from_slice(&self.follows);
        put(&mut bytes, self.why.as_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let rest = bytes.strip_prefix(GAP).ok_or("not a gap fact")?;
        let mut reader = Reader(rest);
        let fact = Self {
            scope: reader.text()?,
            stream: reader.text()?,
            epoch: reader.number()?,
            from: reader.number()?,
            to: reader.number()?,
            start: reader.array()?,
            follows: reader.array()?,
            why: reader.text()?,
        };
        reader.end()?;
        if fact.to < fact.from {
            return Err("a gap that ends before it starts".into());
        }
        Ok(fact)
    }
}
