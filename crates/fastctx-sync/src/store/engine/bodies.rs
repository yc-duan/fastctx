//! Semantic preparation reads bounded bodies before acquiring the writer.
use crate::{
    blocks::codec,
    manifest::{BlockStore, ContentRef, Encoding, ManifestPage},
    store::{Error, access::Custody},
};

pub(super) async fn read(
    blocks: &dyn BlockStore,
    custody: Custody,
    root: &ContentRef,
    max: usize,
) -> Result<Vec<u8>, Error> {
    let mut frontier = vec![root.clone()];
    let mut body = Vec::new();
    let mut metadata = 0_usize;
    while let Some(reference) = frontier.pop() {
        match reference {
            ContentRef::Single { block, decoded_bytes } => {
                if decoded_bytes as usize > max.saturating_sub(body.len()) {
                    return Err(Error::Busy);
                }
                let descriptor = blocks.descriptor(custody.clone(), block.clone()).await?;
                if descriptor.content.decoded_bytes != decoded_bytes {
                    return Err(Error::Corrupt("body extent differs from its reference".into()));
                }
                let representation = descriptor
                    .representations
                    .iter()
                    .find(|r| r.envelope.encoding == Encoding::Raw)
                    .or_else(|| descriptor.representations.first())
                    .ok_or_else(|| Error::Unavailable("body has no verified representation".into()))?;
                let encoded = blocks.read(custody.clone(), block, representation.id()).await?;
                body.extend(codec::decode(representation, encoded)?);
            }
            ContentRef::Manifest { root } => {
                let page = blocks.manifest(custody.clone(), root).await?;
                metadata = metadata.checked_add(page.page_json.len()).ok_or(Error::Busy)?;
                if metadata > max {
                    return Err(Error::Busy);
                }
                match serde_json::from_str::<ManifestPage>(&page.page_json)
                    .map_err(|e| Error::Corrupt(e.to_string()))?
                {
                    ManifestPage::Blocks { offset, blocks } => {
                        if offset.0 != body.len() as u64 {
                            return Err(Error::Corrupt("body manifest has a missing or repeated extent".into()));
                        }
                        frontier.extend(blocks.into_iter().rev().map(ContentRef::from));
                    }
                    ManifestPage::Branch { children } => frontier
                        .extend(children.into_iter().rev().map(|child| ContentRef::Manifest { root: child.root })),
                    ManifestPage::Directory { .. } => {
                        return Err(Error::Invalid("a semantic text body cannot be a directory manifest".into()));
                    }
                }
            }
        }
    }
    Ok(body)
}
