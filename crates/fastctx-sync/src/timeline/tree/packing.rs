//! A candidate can be too large or contain proof wrappers that need separate
//! leaves. Semantic errors remain errors; they are never capacity signals.
use super::*;

pub(super) fn leaf_page(kind: Kind, root: &TimelineRoot, entries: &[Entry]) -> Result<Option<EncodedPage>, Error> {
    if entries.len() > 256 {
        return Ok(None);
    }
    let payload = match kind {
        Kind::Items => {
            let mut items = Vec::new();
            let mut proofs = BTreeMap::new();
            for entry in entries {
                let Entry::Item(item, proof) = entry else { return Err(pages::corrupt("lookup in item tree")) };
                items.push(*item.clone());
                if let Some(old) = proofs.insert(proof.id.clone(), proof.clone())
                    && old != *proof
                {
                    if old.payload.id == proof.payload.id {
                        return Err(Error::Invalid("same proof block has conflicting lengths".into()));
                    }
                    // The proof ID names the signed statement, not the JSON
                    // encoding of its wrapper. Retain both on separate pages.
                    return Ok(None);
                }
            }
            TimelinePayload::Items(Box::new(TimelineLeaf {
                conversation: root.conversation.clone(),
                projection: root.projection,
                through: root.through.clone(),
                items,
                proofs: proofs.into_values().collect(),
            }))
        }
        Kind::Lookup => {
            let mut values = Vec::new();
            for entry in entries {
                let Entry::Lookup(value) = entry else { return Err(pages::corrupt("item in lookup tree")) };
                values.push(value.clone())
            }
            TimelinePayload::ItemLookup(TimelineLookupLeaf { conversation: root.conversation.clone(), entries: values })
        }
    };
    pages::encode_payload(&root.conversation.scope, &payload)
}
pub(super) fn largest(
    max: usize,
    mut build: impl FnMut(usize) -> Result<Option<EncodedPage>, Error>,
) -> Result<(usize, EncodedPage), Error> {
    let mut low = 1;
    let mut high = max;
    let mut best = None;
    while low <= high {
        let mid = low + (high - low) / 2;
        match build(mid)? {
            Some(page) => {
                best = Some((mid, page));
                low = mid + 1
            }
            None => high = mid - 1,
        }
    }
    best.ok_or(Error::Busy)
}
