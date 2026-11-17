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
    /// A capture is the text its sender saw and a deletion of exactly those
    /// characters. Every deleted clock must be one this authority has seen
    /// and none may split a scalar; the scalars still visible among them
    /// must read, in document order, as the frozen body does with whatever
    /// another device deleted meanwhile left out. A character deleted
    /// already is deleted again to no effect, so an edit made elsewhere
    /// while the send was on its way does not refuse the send. An identity
    /// never seen is refused: it would enter Yrs' pending delete set and
    /// delete input that has yet to come.
    pub fn validate_capture(&self, deletion: &DecodedUpdate, expected: &str) -> Result<()> {
        deletion.deletion_units()?;
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
        let mut split = false;
        self.visit_characters(|id, scalar| {
            let Some(ranges) = ranges.get(&yrs::ClientID::new(id.client)) else { return };
            let end = id.clock + scalar.len_utf16() as u32;
            // IdRange stores sorted disjoint ranges. Binary search avoids walking
            // a fragmented deletion set once for every existing character.
            let at = ranges.partition_point(|range| range.end <= id.clock);
            if let Some(range) = ranges.get(at).filter(|range| range.start < end) {
                if range.start > id.clock || range.end < end {
                    split = true;
                } else {
                    selected.push(scalar);
                }
            }
        });
        // `any` takes the body's scalars up to each match: what is still
        // visible is found in the body in order, with gaps where others
        // deleted.
        let mut body = expected.chars();
        if split || !selected.chars().all(|scalar| body.any(|captured| captured == scalar)) {
            return Err(error("the captured deletion does not match the captured draft"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The authority holding `text`, and the deletion a device that has
    /// all of it captures for a send, with the device's replica.
    fn captured(text: &str) -> (Document, Document, DecodedUpdate) {
        let mut authority = Document::new();
        authority.splice(0, 0, text).unwrap();
        let mut device = Document::restore(&authority.checkpoint()).unwrap();
        let taken = Document::restore(&device.checkpoint()).unwrap().take();
        assert_eq!(taken.text, text);
        device.apply(&taken.update).unwrap();
        (authority, device, DecodedUpdate::decode(&taken.update).unwrap())
    }

    #[test]
    fn a_capture_of_what_the_authority_holds_is_taken() {
        let (authority, _, deletion) = captured("abc");
        authority.validate_capture(&deletion, "abc").unwrap();
    }

    #[test]
    fn a_character_another_device_deleted_meanwhile_does_not_refuse_the_send() {
        let (mut authority, _, deletion) = captured("abc");
        authority.splice(2, 3, "").unwrap();
        authority.validate_capture(&deletion, "abc").unwrap();
        authority.splice(0, 2, "").unwrap();
        authority.validate_capture(&deletion, "abc").unwrap();
    }

    #[test]
    fn text_another_device_put_in_meanwhile_stays() {
        let (mut authority, _, deletion) = captured("abc");
        authority.splice(1, 1, "X").unwrap();
        authority.validate_capture(&deletion, "abc").unwrap();
        authority.apply_decoded(deletion).unwrap();
        assert_eq!(authority.text(), "X");
    }

    #[test]
    fn characters_the_authority_never_saw_are_refused() {
        let mut authority = Document::new();
        authority.splice(0, 0, "abc").unwrap();
        let mut device = Document::restore(&authority.checkpoint()).unwrap();
        device.splice(3, 3, "d").unwrap();
        let taken = device.take();
        let deletion = DecodedUpdate::decode(&taken.update).unwrap();
        assert!(authority.validate_capture(&deletion, "abcd").is_err());
    }

    #[test]
    fn a_deletion_of_other_text_than_the_body_is_refused() {
        let (authority, _, deletion) = captured("abc");
        assert!(authority.validate_capture(&deletion, "abd").is_err());
        assert!(authority.validate_capture(&deletion, "cba").is_err());
    }
}
