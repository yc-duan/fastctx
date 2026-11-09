use super::*;
use std::{collections::BTreeSet, future::Future, pin::Pin};

/// Appends one immutable block, preserving all blocks and non-tail pages.
/// Empty blocks preserve an existing extent and can represent an empty new body.
pub async fn append_block(io: &dyn BodyTreeIo, before: Option<&BodyExtent>, block: ContentBlock) -> Result<BodyExtent> {
    Box::pin(append(io, before, block)).await
}

async fn append(io: &dyn BodyTreeIo, before: Option<&BodyExtent>, block: ContentBlock) -> Result<BodyExtent> {
    if block.decoded_bytes > fastctx_types::sync::BLOCK_BYTES {
        return Err(BodyTreeError::InvalidInput("append block exceeds one MiB".into()));
    }
    let Some(before) = before else {
        return Ok(BodyExtent { bytes: ByteCount(u64::from(block.decoded_bytes)), content: block.into() });
    };
    before.content.validate().map_err(BodyTreeError::InvalidInput)?;
    if before.content.id().domain != block.id.domain {
        return Err(BodyTreeError::InvalidInput("append crosses scope".into()));
    }
    if block.decoded_bytes == 0 {
        return Ok(before.clone());
    }
    if before.bytes.0 == 0 {
        if !matches!(&before.content, ContentRef::Single { decoded_bytes: 0, .. }) {
            return Err(corrupt("empty prefix is not an empty single block"));
        }
        return Ok(BodyExtent { bytes: ByteCount(u64::from(block.decoded_bytes)), content: block.into() });
    }
    let scope = block.id.domain.clone();
    let bytes = ByteCount(add(before.bytes.0, u64::from(block.decoded_bytes))?);
    let children = match &before.content {
        ContentRef::Single { block: old, decoded_bytes } => {
            if u64::from(*decoded_bytes) != before.bytes.0 {
                return Err(corrupt("single prefix extent differs"));
            }
            vec![
                publish(
                    io,
                    &scope,
                    ManifestPage::Blocks {
                        offset: ByteCount(0),
                        blocks: vec![ContentBlock { id: old.clone(), decoded_bytes: *decoded_bytes }, block],
                    },
                )
                .await?,
            ]
        }
        ContentRef::Manifest { root } => grow(io, root.clone(), None, before.bytes.0, block, BTreeSet::new()).await?,
    };
    let root = if children.len() == 1 {
        children[0].root.clone()
    } else {
        publish(io, &scope, ManifestPage::Branch { children }).await?.root
    };
    Ok(BodyExtent { content: ContentRef::Manifest { root }, bytes })
}

fn grow<'a>(
    io: &'a dyn BodyTreeIo,
    root: ManifestRoot,
    expected: Option<ManifestChild>,
    offset: u64,
    block: ContentBlock,
    mut visited: BTreeSet<ManifestRoot>,
) -> Pin<Box<dyn Future<Output = Result<Vec<ManifestChild>>> + Send + 'a>> {
    Box::pin(async move {
        if visited.len() >= MAX_DEPTH || !visited.insert(root.clone()) {
            return Err(corrupt("byte tree cycle or depth exceeded"));
        }
        let old = page(io, root.clone()).await?;
        if let Some(child) = &expected {
            verify_child(child, &old)?;
        }
        let (start, end, entries) = extent(&old)?;
        if end != offset || expected.is_none() && start != 0 {
            return Err(corrupt("append prefix extent differs"));
        }
        match old {
            ManifestPage::Blocks { offset: start, mut blocks } => {
                if blocks.len() < FANOUT {
                    blocks.push(block);
                    Ok(vec![publish(io, &root.0.domain, ManifestPage::Blocks { offset: start, blocks }).await?])
                } else {
                    let left = ManifestChild {
                        first_key: key(start.0),
                        last_key: key(end - 1),
                        root: root.clone(),
                        entries: Count(entries),
                    };
                    let right = publish(
                        io,
                        &root.0.domain,
                        ManifestPage::Blocks { offset: ByteCount(offset), blocks: vec![block] },
                    )
                    .await?;
                    Ok(vec![left, right])
                }
            }
            ManifestPage::Branch { mut children } => {
                let tail = children.pop().ok_or_else(|| corrupt("empty branch"))?;
                children.extend(grow(io, tail.root.clone(), Some(tail), offset, block, visited).await?);
                if children.len() <= FANOUT {
                    return Ok(vec![publish(io, &root.0.domain, ManifestPage::Branch { children }).await?]);
                }
                let right = children.split_off(FANOUT / 2);
                Ok(vec![
                    publish(io, &root.0.domain, ManifestPage::Branch { children }).await?,
                    publish(io, &root.0.domain, ManifestPage::Branch { children: right }).await?,
                ])
            }
            _ => Err(corrupt("expected a byte-tree page")),
        }
    })
}
