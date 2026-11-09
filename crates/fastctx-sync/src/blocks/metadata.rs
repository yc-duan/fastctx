//! Registered metadata codecs declare their complete content-reference set.
use crate::{
    catalog::HeadEvidence,
    identity::{Digest, ScopeKey},
    manifest::{BlobId, ContentBlock, ContentRef},
    store::Error,
};
use fastctx_types::sync::{
    PAGE_BYTES,
    conversation_timeline::{ItemDisplayPartsV1, ItemDisplayV1, ProjectedBody},
    cursor::Completeness,
    metadata::{CatalogLeaf, HistoryLeaf, MetadataCodec, ProofReference, TimelinePayload},
};
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub(crate) fn validate(
    scope: &ScopeKey,
    codec: MetadataCodec,
    payload: &ContentBlock,
    bytes: &[u8],
    declared: &[ContentRef],
) -> Result<(), Error> {
    if payload.id.domain != *scope
        || payload.decoded_bytes as usize != bytes.len()
        || bytes.len() > PAGE_BYTES as usize
        || Digest::of(bytes) != payload.id.digest
    {
        return Err(invalid("index payload differs from its bounded content binding"));
    }
    let references = match codec {
        MetadataCodec::HistoryV1 => history(scope, decode(bytes)?)?,
        MetadataCodec::CatalogV1 => catalog(scope, decode(bytes)?)?,
        MetadataCodec::TimelineV1 => timeline_references(scope, &decode(bytes)?)?,
        MetadataCodec::DisplayV1 => {
            let display = ItemDisplayV1::decode(bytes).map_err(Error::Invalid)?;
            display.validate_scopes(scope).map_err(Error::Invalid)?;
            display.owned_content()
        }
        MetadataCodec::DisplayPartsV1 => {
            let page = ItemDisplayPartsV1::decode(bytes).map_err(Error::Invalid)?;
            page.validate_scopes(scope).map_err(Error::Invalid)?;
            page.owned_content()
        }
        MetadataCodec::BodyV1 => decode::<ProjectedBody>(bytes)?.owned_content().into_iter().cloned().collect(),
    };
    if set(scope, references)? != set(scope, declared.to_vec())? {
        return Err(invalid("index payload and declared references differ"));
    }
    Ok(())
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, Error> {
    serde_json::from_slice(bytes).map_err(|_| invalid("index payload does not match its registered codec"))
}
fn invalid(reason: &str) -> Error {
    Error::Invalid(reason.into())
}
fn set(scope: &ScopeKey, values: Vec<ContentRef>) -> Result<BTreeMap<(BlobId, bool), ContentRef>, Error> {
    let mut output = BTreeMap::new();
    for reference in values {
        reference.validate().map_err(Error::Invalid)?;
        if reference.id().domain != *scope {
            return Err(invalid("index reference crosses its scope"));
        }
        let key = (reference.id().clone(), matches!(reference, ContentRef::Manifest { .. }));
        if let Some(previous) = output.insert(key, reference.clone())
            && previous != reference
        {
            return Err(invalid("index references disagree on a content length"));
        }
    }
    Ok(output)
}
fn proof_refs(proofs: &[ProofReference]) -> Result<(BTreeSet<Digest>, Vec<ContentRef>), Error> {
    let mut ids = BTreeSet::new();
    let mut references = Vec::new();
    for proof in proofs {
        if proof.payload.decoded_bytes > PAGE_BYTES || !ids.insert(proof.id.clone()) {
            return Err(invalid("index repeats a proof identity or exceeds its proof budget"));
        }
        references.push(ContentRef::from(proof.payload.clone()));
    }
    Ok((ids, references))
}
fn history(scope: &ScopeKey, page: HistoryLeaf) -> Result<Vec<ContentRef>, Error> {
    if page.object.scope != *scope || page.states.is_empty() || page.states.len() > 64 {
        return Err(invalid("history index needs one to 64 scoped predecessor states"));
    }
    let mut versions = HashSet::new();
    let mut references = Vec::new();
    for state in page.states {
        if state.version.revision.0 == 0
            || state.current_version.revision.0 == 0
            || !versions.insert(state.version)
            || state.proof.payload.decoded_bytes > PAGE_BYTES
        {
            return Err(invalid("history index repeats or misidentifies a predecessor state"));
        }
        references.push(ContentRef::from(state.proof.payload));
        references.extend(state.candidate);
        references.extend(state.current_content);
    }
    if let Some(root) = page.previous {
        references.push(ContentRef::Manifest { root });
    }
    Ok(references)
}
fn catalog(scope: &ScopeKey, page: CatalogLeaf) -> Result<Vec<ContentRef>, Error> {
    if page.cursor.scope != *scope
        || page.cursor.view.is_none()
        || page.cursor.prefix_digest.is_none()
        || page.objects.len() > 256
    {
        return Err(invalid("catalog leaf has no scoped visibility fence or exceeds its object budget"));
    }
    let (proofs, references) = proof_refs(&page.proofs)?;
    let mut used = BTreeSet::new();
    let mut previous = None;
    for head in page.objects {
        if head.key.scope != *scope
            || previous.as_ref().is_some_and(|key| key >= &head.key.object)
            || head.acl_revision != page.cursor.acl_revision
            || !matches!(head.completeness, Completeness::MetadataOnly)
        {
            return Err(invalid("catalog leaf must contain sorted scoped heads without local completeness claims"));
        }
        previous = Some(head.key.object);
        match head.evidence {
            HeadEvidence::Accepted { proof } => {
                if !proofs.contains(&proof) {
                    return Err(invalid("catalog head has no declared original acceptance proof"));
                }
                used.insert(proof);
            }
            HeadEvidence::Stream { .. } => {}
        }
        // Object bodies are descriptions, not dependencies owned by the index.
        // Replica installation binds them to each actual object and kind.
        for root in head.content.iter() {
            root.validate().map_err(Error::Invalid)?;
            if root.id().domain != *scope {
                return Err(invalid("catalog description crosses its scope"));
            }
        }
        if head.history.as_ref().is_some_and(|root| root.0.domain != *scope) {
            return Err(invalid("catalog history description crosses its scope"));
        }
    }
    if proofs != used {
        return Err(invalid("catalog leaf carries an unrelated acceptance proof"));
    }
    Ok(references)
}
pub(crate) fn timeline_references(scope: &ScopeKey, payload: &TimelinePayload) -> Result<Vec<ContentRef>, Error> {
    use TimelinePayload::*;
    let page = match payload {
        Root(root) => {
            if root.conversation.scope != *scope
                || root.through.scope != *scope
                || root.projection.0 == 0
                || root.items.is_some() != root.lookup.is_some()
                || root.items.as_ref().map(|c| c.entries) != root.lookup.as_ref().map(|c| c.entries)
            {
                return Err(invalid("timeline root has inconsistent scope, projection or tree counts"));
            }
            let mut refs = Vec::new();
            for child in root.items.iter().chain(root.lookup.iter()) {
                if child.entries.0 == 0 || child.first_key > child.last_key || child.root.0.domain != *scope {
                    return Err(invalid("timeline root has an invalid child range"));
                }
                refs.push(ContentRef::Manifest { root: child.root.clone() });
            }
            return Ok(refs);
        }
        ItemLookup(page) => {
            if page.conversation.scope != *scope || page.entries.is_empty() || page.entries.len() > 256 {
                return Err(invalid("timeline lookup has an invalid scope or item bound"));
            }
            let mut previous = None;
            for entry in &page.entries {
                if entry.item != entry.position.item
                    || entry.revision.0 == 0
                    || entry.position.display_ancestry.is_empty()
                    || previous.is_some_and(|id| id >= &entry.item)
                {
                    return Err(invalid("timeline lookup is unsorted or has an invalid location"));
                }
                previous = Some(&entry.item);
            }
            return Ok(Vec::new());
        }
        Items(page) => page,
    };
    if page.conversation.scope != *scope
        || page.through.scope != *scope
        || page.projection.0 == 0
        || page.items.len() > 256
    {
        return Err(invalid("timeline leaf has an invalid scope, projection or item bound"));
    }
    let (proofs, mut references) = proof_refs(&page.proofs)?;
    let mut used = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut previous = None;
    for entry in &page.items {
        let item = &entry.item;
        if item.position.item != item.id
            || item.revision.0 == 0
            || item.position.display_ancestry.is_empty()
            || !ids.insert(item.id.clone())
            || previous.as_ref().is_some_and(|position| position >= &item.position)
            || entry.record.sequence.0 == 0
            || (entry.record.stream == page.through.stream
                && entry.record.epoch == page.through.epoch
                && entry.record.sequence > page.through.committed)
            || !proofs.contains(&entry.batch)
        {
            return Err(invalid("timeline index is unsorted, duplicated or lacks its source evidence"));
        }
        item.display.validate().map_err(Error::Invalid)?;
        item.display.validate_scopes(scope).map_err(Error::Invalid)?;
        used.insert(entry.batch.clone());
        previous = Some(item.position.clone());
        references.push(ContentRef::from(entry.payload.clone()));
        references.push(item.body.clone());
        references.extend(item.display.owned_content());
    }
    if proofs != used {
        return Err(invalid("timeline leaf carries an unrelated source proof"));
    }
    Ok(references)
}
