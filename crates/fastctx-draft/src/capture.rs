//! Prepared updates are decoded before entering an editor's critical section.
use super::{Document, Result, error};
use yrs::{ReadTxn, Transact, Update, updates::decoder::Decode};

/// A decoded v1 update. Its owner can validate it and integrate it exactly once.
pub struct DecodedUpdate(pub(super) Update);
impl DecodedUpdate {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        Update::decode_v1(bytes).map(Self).map_err(error)
    }

    /// Validates a delete-only update and counts its UTF-16 clock units.
    pub fn deletion_units(&self) -> Result<u64> {
        if !self.0.insertions(true).is_empty() {
            return Err(error("the captured deletion contains inserted data"));
        }
        let mut units = 0u64;
        for (_, ranges) in self.0.delete_set().iter() {
            let mut end = 0;
            for range in ranges {
                if range.start < end || range.start >= range.end {
                    return Err(error("the captured deletion range is invalid"));
                }
                units = units
                    .checked_add(u64::from(range.end - range.start))
                    .ok_or_else(|| error("the captured deletion length overflowed"))?;
                end = range.end;
            }
        }
        Ok(units)
    }
}

impl Document {
    /// Requires every deleted clock to be a visible scalar of this authority,
    /// and their document-order text to equal the frozen body. Unknown identities
    /// must never enter Yrs' pending delete set and pre-delete future input.
    pub fn validate_capture(&self, deletion: &DecodedUpdate, expected: &str) -> Result<()> {
        let units = deletion.deletion_units()?;
        let transaction = self.doc.transact();
        let vector = transaction.state_vector();
        for (client, ranges) in deletion.0.delete_set().iter() {
            if ranges.into_iter().any(|range| range.end > vector.get(client)) {
                return Err(error("the captured deletion references unknown characters"));
            }
        }
        let ranges: std::collections::HashMap<_, Vec<_>> = deletion
            .0
            .delete_set()
            .iter()
            .map(|(client, ranges)| (*client, ranges.into_iter().cloned().collect()))
            .collect();
        let mut selected = String::new();
        let mut matched = 0u64;
        let mut split = false;
        self.visit_characters(|id, scalar| {
            let Some(ranges) = ranges.get(&yrs::ClientID::new(id.client)) else { return };
            let length = scalar.len_utf16() as u32;
            let end = id.clock + length;
            // IdRange stores sorted disjoint ranges. Binary search avoids walking
            // a fragmented deletion set once for every existing character.
            let at = ranges.partition_point(|range| range.end <= id.clock);
            if let Some(range) = ranges.get(at).filter(|range| range.start < end) {
                if range.start > id.clock || range.end < end {
                    split = true;
                } else {
                    selected.push(scalar);
                    matched += u64::from(length);
                }
            }
        });
        if split || matched != units || selected != expected {
            return Err(error("the captured deletion does not match the captured draft"));
        }
        Ok(())
    }
}
