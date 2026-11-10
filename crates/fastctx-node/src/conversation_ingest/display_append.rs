//! Copy-on-write extension of public display descriptions. Only the right-hand
//! path changes; existing part bodies are held by committed source facts.
use super::{
    display_pages::{DescriptorPage, join_parts},
    step_content::StepContent,
};
use fastctx_sync::{
    cursor::Count,
    manifest::{ContentRef, ManifestPage, ManifestRoot},
    store::Error,
};
use fastctx_types::sync::{conversation_timeline::*, metadata::MetadataCodec};

pub async fn load(io: &StepContent<'_>, content: &ContentRef) -> Result<ItemDisplayV1, Error> {
    let ContentRef::Manifest { root } = content else {
        return Err(Error::Corrupt("public display is not a Metadata page".into()));
    };
    let page = io.public_manifest(root.clone()).await?;
    match serde_json::from_str(&page.page_json).map_err(json)? {
        ManifestPage::Metadata { codec: MetadataCodec::DisplayV1, payload, .. } => {
            ItemDisplayV1::decode(&io.public_block(payload).await?).map_err(Error::Corrupt)
        }
        _ => Err(Error::Corrupt("public display has a different codec".into())),
    }
}
pub async fn publish(io: &StepContent<'_>, display: &ItemDisplayV1) -> Result<ContentRef, Error> {
    let plan = DescriptorPage::display(io.target().target().custody.object.scope.clone(), display).map_err(invalid)?;
    let block = io.put_block(plan.payload().to_vec()).await?;
    let page = plan.manifest(block).map_err(invalid)?;
    let root = page.root.clone();
    io.put_manifest(page).await?;
    Ok(ContentRef::Manifest { root })
}
pub fn count(display: &ItemDisplayV1) -> u64 {
    display.parts.len() as u64 + display.more.as_ref().map_or(0, |more| more.count.0)
}
pub async fn append(io: &StepContent<'_>, display: &mut ItemDisplayV1, mut part: DisplayPart) -> Result<(), Error> {
    part.index = Count(count(display));
    let used = display.parts.iter().filter_map(|p| p.preview.as_ref()).map(String::len).sum::<usize>();
    if let Some(preview) = &mut part.preview {
        *preview = display_preview(preview, 4096usize.saturating_sub(used));
    }
    part.validate().map_err(Error::Invalid)?;
    if display.more.is_none() && display.parts.len() < 4 {
        loop {
            display.parts.push(part.clone());
            if display.encode().is_ok_and(|v| v.len() <= 16384 - 512) {
                return Ok(());
            }
            display.parts.pop();
            let Some(preview) = &mut part.preview else { break };
            if preview.is_empty() {
                break;
            }
            *preview = display_preview(preview, preview.len() / 2);
        }
    }
    part.preview = None;
    let roots = match display.more.take() {
        Some(root) => extend(io, root, part, 0).await?,
        None => vec![leaf(io, vec![part]).await?],
    };
    display.more = Some(join(io, &roots).await?);
    Ok(())
}
async fn extend(
    io: &StepContent<'_>,
    root: DisplayPartsRef,
    part: DisplayPart,
    depth: usize,
) -> Result<Vec<DisplayPartsRef>, Error> {
    if depth >= 64 || part.index.0 != root.start.0.checked_add(root.count.0).ok_or(Error::Busy)? {
        return Err(Error::Corrupt("display overflow extent is inconsistent".into()));
    }
    let page = io.public_manifest(root.root.clone()).await?;
    match serde_json::from_str(&page.page_json).map_err(json)? {
        ManifestPage::Metadata { codec: MetadataCodec::DisplayPartsV1, payload, .. } => {
            let mut old = ItemDisplayPartsV1::decode(&io.public_block(payload).await?).map_err(Error::Corrupt)?;
            if old.start != root.start || old.parts.len() as u64 != root.count.0 {
                return Err(Error::Corrupt("display leaf range differs".into()));
            }
            old.parts.push(part.clone());
            if old.parts.len() <= 8 && old.encode().is_ok() {
                Ok(vec![leaf(io, old.parts).await?])
            } else {
                Ok(vec![root, leaf(io, vec![part]).await?])
            }
        }
        ManifestPage::Branch { children } => {
            let mut refs = children
                .into_iter()
                .map(|c| {
                    Ok(DisplayPartsRef {
                        start: Count(u64::from_str_radix(&c.first_key, 16).map_err(|e| Error::Corrupt(e.to_string()))?),
                        count: c.entries,
                        root: c.root,
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            let last = refs.pop().ok_or_else(|| Error::Corrupt("empty display branch".into()))?;
            refs.extend(Box::pin(extend(io, last, part, depth + 1)).await?);
            if refs.len() <= 32 {
                Ok(vec![join(io, &refs).await?])
            } else {
                let suffix = refs.split_off(32);
                Ok(vec![join(io, &refs).await?, join(io, &suffix).await?])
            }
        }
        _ => Err(Error::Corrupt("display overflow has a different page shape".into())),
    }
}
async fn leaf(io: &StepContent<'_>, parts: Vec<DisplayPart>) -> Result<DisplayPartsRef, Error> {
    let start = parts.first().ok_or(Error::Busy)?.index;
    let plan = DescriptorPage::parts(
        io.target().target().custody.object.scope.clone(),
        &ItemDisplayPartsV1 { version: 1, start, parts },
    )
    .map_err(invalid)?;
    let block = io.put_block(plan.payload().to_vec()).await?;
    let (page, root) = plan.parts_manifest(block).map_err(invalid)?;
    io.put_manifest(page).await?;
    Ok(root)
}
async fn join(io: &StepContent<'_>, parts: &[DisplayPartsRef]) -> Result<DisplayPartsRef, Error> {
    let joined = join_parts(&io.target().target().custody.object.scope, parts).map_err(invalid)?;
    if let Some(page) = joined.page {
        io.put_manifest(page).await?;
    }
    Ok(joined.reference)
}
pub async fn part(io: &StepContent<'_>, display: &ItemDisplayV1, index: Count) -> Result<DisplayPart, Error> {
    if let Some(part) = display.parts.get(index.0 as usize) {
        return Ok(part.clone());
    }
    let mut root: ManifestRoot = display.more.as_ref().ok_or(Error::Busy)?.root.clone();
    for _ in 0..64 {
        let page = io.public_manifest(root).await?;
        match serde_json::from_str(&page.page_json).map_err(json)? {
            ManifestPage::Metadata { codec: MetadataCodec::DisplayPartsV1, payload, .. } => {
                let leaf = ItemDisplayPartsV1::decode(&io.public_block(payload).await?).map_err(Error::Corrupt)?;
                return index
                    .0
                    .checked_sub(leaf.start.0)
                    .and_then(|i| leaf.parts.get(i as usize))
                    .cloned()
                    .ok_or_else(|| Error::Corrupt("part is outside its leaf".into()));
            }
            ManifestPage::Branch { children } => {
                let key = format!("{:016x}", index.0);
                root = children
                    .into_iter()
                    .find(|c| c.first_key <= key && key <= c.last_key)
                    .ok_or_else(|| Error::Corrupt("part is absent from display branch".into()))?
                    .root;
            }
            _ => return Err(Error::Corrupt("invalid display overflow page".into())),
        }
    }
    Err(Error::Corrupt("display overflow exceeds its depth bound".into()))
}
fn invalid(e: std::io::Error) -> Error {
    Error::Invalid(e.to_string())
}
fn json(e: serde_json::Error) -> Error {
    Error::Corrupt(e.to_string())
}
