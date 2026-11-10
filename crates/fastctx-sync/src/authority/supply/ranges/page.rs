//! Select an ordered prefix while decoding; wide branches never allocate a
//! second complete children vector just to return the first bounded interval.
use super::*;
use crate::manifest::{ContentBlock, ManifestChild};
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};

pub(super) enum Selected {
    Blocks { count: u64, blocks: Vec<IndexedBlock> },
    Branch { count: u64, children: Vec<(ManifestRoot, u64, Option<u64>)> },
}
impl Selected {
    pub fn count(&self) -> u64 {
        match self {
            Self::Blocks { count, .. } | Self::Branch { count, .. } => *count,
        }
    }
}
struct Select<'a> {
    start: u64,
    missing: &'a [BlockRange],
    limit: usize,
}
impl Select<'_> {
    fn intersects(&self, start: u64, end: u64) -> bool {
        self.missing.iter().any(|range| range.start.0 < end && start < range.end.0)
    }
}
pub(super) fn decode(bytes: &[u8], start: u64, missing: &[BlockRange], limit: usize) -> Result<Selected, Error> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let page = Select { start, missing, limit }
        .deserialize(&mut decoder)
        .map_err(|error| Error::Corrupt(format!("invalid stored ordered manifest: {error}")))?;
    decoder.end().map_err(|error| Error::Corrupt(error.to_string()))?;
    Ok(page)
}
impl<'de> DeserializeSeed<'de> for Select<'_> {
    type Value = Selected;
    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Selected, D::Error> {
        decoder.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for Select<'_> {
    type Value = Selected;
    fn expecting(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("ordered manifest page")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Selected, A::Error> {
        let mut tag = None;
        let mut selected = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" if tag.is_none() => tag = Some(map.next_value::<String>()?),
                "blocks" if selected.is_none() => selected = Some(map.next_value_seed(Blocks(&self))?),
                "children" if selected.is_none() => selected = Some(map.next_value_seed(Children(&self))?),
                "offset" => {
                    map.next_value::<crate::cursor::ByteCount>()?;
                }
                _ => {
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
            }
        }
        match (tag.as_deref(), selected) {
            (Some("blocks"), Some(value @ Selected::Blocks { .. }))
            | (Some("branch"), Some(value @ Selected::Branch { .. })) => Ok(value),
            _ => Err(serde::de::Error::custom("ranges require Blocks/Branch")),
        }
    }
}
struct Blocks<'a, 'b>(&'a Select<'b>);
impl<'de> DeserializeSeed<'de> for Blocks<'_, '_> {
    type Value = Selected;
    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Selected, D::Error> {
        decoder.deserialize_seq(self)
    }
}
impl<'de> Visitor<'de> for Blocks<'_, '_> {
    type Value = Selected;
    fn expecting(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("ordered blocks")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Selected, A::Error> {
        let mut count = 0_u64;
        let mut blocks = Vec::new();
        while let Some(content) = seq.next_element::<ContentBlock>()? {
            let index =
                self.0.start.checked_add(count).ok_or_else(|| serde::de::Error::custom("block ordinal overflow"))?;
            let end = index.checked_add(1).ok_or_else(|| serde::de::Error::custom("block ordinal overflow"))?;
            if blocks.len() < self.0.limit && self.0.intersects(index, end) {
                blocks.push(IndexedBlock { index: Count(index), content });
            }
            count = count.checked_add(1).ok_or_else(|| serde::de::Error::custom("block count overflow"))?;
        }
        Ok(Selected::Blocks { count, blocks })
    }
}
struct Children<'a, 'b>(&'a Select<'b>);
impl<'de> DeserializeSeed<'de> for Children<'_, '_> {
    type Value = Selected;
    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Selected, D::Error> {
        decoder.deserialize_seq(self)
    }
}
impl<'de> Visitor<'de> for Children<'_, '_> {
    type Value = Selected;
    fn expecting(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("ordered child ranges")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Selected, A::Error> {
        let mut next = self.0.start;
        let mut children = Vec::new();
        let mut previous = None;
        while let Some(child) = seq.next_element::<ManifestChild>()? {
            if child.first_key > child.last_key || previous.as_ref().is_some_and(|key| key >= &child.first_key) {
                return Err(serde::de::Error::custom("unordered stored child ranges"));
            }
            previous = Some(child.last_key);
            let end = next
                .checked_add(child.entries.0)
                .ok_or_else(|| serde::de::Error::custom("manifest ordinal overflow"))?;
            if children.len() < self.0.limit && self.0.intersects(next, end) {
                children.push((child.root, next, Some(child.entries.0)));
            }
            next = end;
        }
        Ok(Selected::Branch { count: next - self.0.start, children })
    }
}
