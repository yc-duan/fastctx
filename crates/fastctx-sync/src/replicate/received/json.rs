//! Count the JSON structure without building it before admitting typed decoding.
use crate::{admission::ByteBudget, replicate::Received, store::Error, transfer::Message};
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};

// Covers v1 typed vector elements, serde's tagged-enum intermediate nodes and
// BTreeMap leaf slack. Strings and parser scratch are charged separately.
const NODE_BYTES: usize = 2048;
pub fn decode_message(bytes: &[u8], budget: &ByteBudget) -> Result<Received<Message>, Error> {
    decode_json(bytes, budget)
}
/// Admits typed JSON by structure before allocating the typed value.
pub fn decode_json<T: serde::de::DeserializeOwned>(bytes: &[u8], budget: &ByteBudget) -> Result<Received<T>, Error> {
    let _scratch = budget.try_acquire(bytes.len().saturating_mul(2).saturating_add(32 * 1024))?;
    let mut cost = Cost { bytes: 0, limit: budget.capacity(), exhausted: false };
    let counted = {
        let mut decoder = serde_json::Deserializer::from_slice(bytes);
        Seed(&mut cost, false).deserialize(&mut decoder).and_then(|()| decoder.end())
    };
    if let Err(error) = counted {
        return if cost.exhausted { Err(Error::Busy) } else { Err(Error::Invalid(error.to_string())) };
    }
    let permit = budget.try_acquire(cost.bytes)?;
    let value = serde_json::from_slice(bytes).map_err(|_| Error::Invalid("invalid asset metadata message".into()))?;
    Ok(Received::new(value, permit))
}
struct Cost {
    bytes: usize,
    limit: usize,
    exhausted: bool,
}
impl Cost {
    fn add<E: serde::de::Error>(&mut self, bytes: usize) -> Result<(), E> {
        match self.bytes.checked_add(bytes).filter(|n| *n <= self.limit) {
            Some(n) => {
                self.bytes = n;
                Ok(())
            }
            None => {
                self.exhausted = true;
                Err(E::custom("asset semantic working set exceeds its runtime pool"))
            }
        }
    }
}
struct Seed<'a>(&'a mut Cost, bool);
impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<(), D::Error> {
        if !self.1 {
            self.0.add(NODE_BYTES)?;
        }
        decoder.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Seed<'_> {
    type Value = ();
    fn expecting(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("bounded JSON")
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<(), E> {
        self.0.add(text.len().saturating_mul(8))
    }
    fn visit_borrowed_str<E: serde::de::Error>(self, text: &'de str) -> Result<(), E> {
        self.visit_str(text)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while seq.next_element_seed(Seed(self.0, false))?.is_some() {}
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while map.next_key_seed(Seed(self.0, true))?.is_some() {
            map.next_value_seed(Seed(self.0, false))?;
        }
        Ok(())
    }
}
