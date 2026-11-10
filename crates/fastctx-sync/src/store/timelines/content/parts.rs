//! Part-index branches expose descriptor coordinates rather than storage pages.
use crate::{cursor::Count, manifest::ManifestPage, store::Error};
use fastctx_types::sync::conversation_timeline::{DisplayPartsRef, ItemDisplayPartsBranchV1};

pub(super) fn logical(page: &ManifestPage) -> Result<Option<Vec<u8>>, Error> {
    let ManifestPage::Branch { children } = page else { return Ok(None) };
    if children.first().is_none_or(|child| child.first_key.len() != 16) {
        return Ok(None);
    }
    let mut descriptors = Vec::with_capacity(children.len());
    let mut end = None;
    for child in children {
        let start = index(&child.first_key)?;
        let last = index(&child.last_key)?;
        let next = last.checked_add(1).ok_or_else(|| corrupt("part index overflow"))?;
        let count = next.checked_sub(start).ok_or_else(|| corrupt("reversed part range"))?;
        if count == 0 || child.entries != Count(count) || end.is_some_and(|end| end != start) {
            return Err(corrupt("part branch count or contiguous coverage differs"));
        }
        end = Some(next);
        descriptors.push(DisplayPartsRef { root: child.root.clone(), start: Count(start), count: Count(count) });
    }
    let start = descriptors.first().ok_or_else(|| corrupt("empty part branch"))?.start;
    let value = ItemDisplayPartsBranchV1 {
        version: 1,
        start,
        count: Count(end.ok_or_else(|| corrupt("empty part branch"))? - start.0),
        children: descriptors,
    };
    serde_json::to_vec(&value).map(Some).map_err(|error| Error::Corrupt(error.to_string()))
}
fn index(key: &str) -> Result<u64, Error> {
    let value = u64::from_str_radix(key, 16).map_err(|_| corrupt("invalid part index"))?;
    if key != format!("{value:016x}") {
        return Err(corrupt("noncanonical part index"));
    }
    Ok(value)
}
fn corrupt(reason: &str) -> Error {
    Error::Corrupt(reason.into())
}
