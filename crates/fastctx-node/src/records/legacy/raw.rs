//! Preserve undeclared fields and exact numeric tokens while validating the
//! known shape. Canonical hashes ignore object order and JSON whitespace only.
use serde::{Serialize, de::DeserializeOwned};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use std::io;

pub(crate) fn validate<T: DeserializeOwned>(value: &RawValue) -> io::Result<()> {
    let _: T = serde_json::from_str(value.get())?;
    Ok(())
}

/// Applies a known format conversion while retaining undeclared object fields.
pub(crate) fn merge(bytes: &[u8], value: &impl Serialize) -> io::Result<Vec<u8>> {
    fn overlay(old: &RawValue, new: Box<RawValue>, depth: usize) -> io::Result<Box<RawValue>> {
        nesting(depth)?;
        match (old.get().as_bytes().first(), new.get().as_bytes().first()) {
            (Some(b'{'), Some(b'{')) => {
                let mut original: Fields = serde_json::from_str(old.get())?;
                for (key, value) in serde_json::from_str::<Fields>(new.get())?.0 {
                    let value = match original.0.iter().position(|(name, _)| name == &key) {
                        Some(index) => overlay(&original.0.remove(index).1, value, depth + 1)?,
                        None => value,
                    };
                    original.0.retain(|(name, _)| name != &key);
                    original.0.push((key, value));
                }
                serde_json::value::to_raw_value(&original).map_err(io::Error::other)
            }
            (Some(b'['), Some(b'[')) => {
                let old: Vec<Box<RawValue>> = serde_json::from_str(old.get())?;
                let values: Vec<Box<RawValue>> = serde_json::from_str(new.get())?;
                if old.len() != values.len() {
                    return Ok(new);
                }
                let values = old
                    .into_iter()
                    .zip(values)
                    .map(|(old, new)| overlay(&old, new, depth + 1))
                    .collect::<io::Result<Vec<_>>>()?;
                serde_json::value::to_raw_value(&values).map_err(io::Error::other)
            }
            _ => Ok(new),
        }
    }
    let original: Box<RawValue> = serde_json::from_slice(bytes)?;
    let merged = overlay(&original, serde_json::value::to_raw_value(value)?, 0)?;
    Ok(merged.get().as_bytes().to_vec())
}

pub(crate) fn digest(bytes: &[u8]) -> io::Result<String> {
    fn add(hash: &mut Sha256, value: &RawValue, depth: usize) -> io::Result<()> {
        nesting(depth)?;
        let text = value.get().trim();
        match text.as_bytes().first() {
            Some(b'{') => {
                let mut fields: Fields = serde_json::from_str(text)?;
                fields.0.sort_by(|left, right| left.0.cmp(&right.0));
                hash.update(b"{");
                for (key, value) in fields.0 {
                    let key = serde_json::to_vec(&key)?;
                    hash.update((key.len() as u64).to_be_bytes());
                    hash.update(key);
                    add(hash, &value, depth + 1)?;
                }
                hash.update(b"}");
            }
            Some(b'[') => {
                let values: Vec<Box<RawValue>> = serde_json::from_str(text)?;
                hash.update(b"[");
                for value in values {
                    add(hash, &value, depth + 1)?;
                }
                hash.update(b"]");
            }
            Some(b'"') => {
                let text: String = serde_json::from_str(text)?;
                hash.update(b"s");
                hash.update((text.len() as u64).to_be_bytes());
                hash.update(text.as_bytes());
            }
            _ => {
                hash.update(b"v");
                hash.update((text.len() as u64).to_be_bytes());
                hash.update(text.as_bytes());
            }
        }
        Ok(())
    }
    let raw: Box<RawValue> = serde_json::from_slice(bytes)?;
    let mut hash = Sha256::new();
    add(&mut hash, &raw, 0)?;
    Ok(format!("{:x}", hash.finalize()))
}

// Repeated undeclared keys remain distinct; a map would silently drop them.
struct Fields(Vec<(String, Box<RawValue>)>);
impl<'de> serde::Deserialize<'de> for Fields {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct Entries;
        impl<'de> serde::de::Visitor<'de> for Entries {
            type Value = Fields;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Fields, A::Error> {
                let mut fields = Vec::new();
                while let Some(field) = map.next_entry()? {
                    fields.push(field);
                }
                Ok(Fields(fields))
            }
        }
        decoder.deserialize_map(Entries)
    }
}
impl Serialize for Fields {
    fn serialize<S: serde::Serializer>(&self, encoder: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = encoder.serialize_map(Some(self.0.len()))?;
        for (name, value) in &self.0 {
            map.serialize_entry(name, value)?;
        }
        map.end()
    }
}

fn nesting(depth: usize) -> io::Result<()> {
    // Raw values bypass typed traversal, so recursive work is bounded here.
    if depth > 128 {
        return Err(crate::record_migration::errors::invalid("legacy JSON nesting exceeds its stack guard"));
    }
    Ok(())
}
