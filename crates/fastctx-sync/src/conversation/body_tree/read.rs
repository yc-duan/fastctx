use super::*;
use std::collections::BTreeSet;

/// Locates a block by absolute byte offset, checking every followed child binding.
pub async fn locate(io: &dyn BodyTreeIo, body: &BodyExtent, offset: ByteCount) -> Result<(ByteCount, ContentBlock)> {
    Box::pin(locate_inner(io, body, offset)).await
}

async fn locate_inner(io: &dyn BodyTreeIo, body: &BodyExtent, offset: ByteCount) -> Result<(ByteCount, ContentBlock)> {
    body.content.validate().map_err(BodyTreeError::InvalidInput)?;
    if offset.0 >= body.bytes.0 {
        return Err(BodyTreeError::InvalidInput("offset outside body extent".into()));
    }
    let mut root = match &body.content {
        ContentRef::Single { block, decoded_bytes } => {
            if u64::from(*decoded_bytes) != body.bytes.0 {
                return Err(corrupt("single body extent differs"));
            }
            return Ok((ByteCount(0), ContentBlock { id: block.clone(), decoded_bytes: *decoded_bytes }));
        }
        ContentRef::Manifest { root } => root.clone(),
    };
    let mut expected = None;
    let mut visited = BTreeSet::new();
    for _ in 0..MAX_DEPTH {
        if !visited.insert(root.clone()) {
            return Err(corrupt("byte tree cycle"));
        }
        let current = page(io, root.clone()).await?;
        if let Some(child) = &expected {
            verify_child(child, &current)?;
        }
        let (start, end, _) = extent(&current)?;
        if expected.is_none() && (start != 0 || end != body.bytes.0) {
            return Err(corrupt("body root extent differs"));
        }
        match current {
            ManifestPage::Blocks { offset: start, blocks } => {
                let mut at = start.0;
                for block in blocks {
                    let end = add(at, u64::from(block.decoded_bytes))?;
                    if at <= offset.0 && offset.0 < end {
                        return Ok((ByteCount(at), block));
                    }
                    at = end;
                }
                return Err(corrupt("body leaf does not cover offset"));
            }
            ManifestPage::Branch { children } => {
                let child = children
                    .into_iter()
                    .find(|child| range(child).is_ok_and(|(first, end)| first <= offset.0 && offset.0 < end))
                    .ok_or_else(|| corrupt("branch does not cover offset"))?;
                root = child.root.clone();
                expected = Some(child);
            }
            _ => return Err(corrupt("expected a byte-tree page")),
        }
    }
    Err(corrupt("byte tree depth exceeded"))
}

/// Reads only the blocks intersecting a bounded range; content hashes are checked.
pub async fn read_range(
    io: &dyn BodyTreeIo,
    body: &BodyExtent,
    offset: ByteCount,
    bytes: usize,
    max_bytes: usize,
) -> Result<Vec<u8>> {
    Box::pin(read_inner(io, body, offset, bytes, max_bytes)).await
}

async fn read_inner(
    io: &dyn BodyTreeIo,
    body: &BodyExtent,
    offset: ByteCount,
    bytes: usize,
    max_bytes: usize,
) -> Result<Vec<u8>> {
    if bytes > max_bytes {
        return Err(BodyTreeError::Budget { needed: bytes, available: max_bytes });
    }
    let end = add(offset.0, bytes as u64)?;
    if end > body.bytes.0 {
        return Err(BodyTreeError::InvalidInput("read exceeds body extent".into()));
    }
    let mut output = Vec::with_capacity(bytes);
    let mut at = offset.0;
    while at < end {
        let (start, block) = locate(io, body, ByteCount(at)).await?;
        let decoded = io.read_block(block.clone()).await?;
        if decoded.len() != block.decoded_bytes as usize || Digest::of(&decoded) != block.id.digest {
            return Err(corrupt("body block identity or length differs"));
        }
        let local = usize::try_from(at - start.0).map_err(|_| corrupt("block offset overflow"))?;
        let available = decoded.len().checked_sub(local).ok_or_else(|| corrupt("block offset differs"))?;
        let take = available.min((end - at) as usize);
        if take == 0 {
            return Err(corrupt("body range made no progress"));
        }
        output.extend_from_slice(&decoded[local..local + take]);
        at = add(at, take as u64)?;
    }
    Ok(output)
}
