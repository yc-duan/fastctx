use super::{api::*, state::Engine};
use crate::{
    conversation::{
        AppendPlan, ItemState,
        body_tree::{self, BodyExtent, BodyTreeIo},
    },
    cursor::ByteCount,
    manifest::{ContentRef, ManifestPage},
};
use fastctx_types::sync::{conversation_timeline::*, metadata::MetadataCodec};

impl Engine<'_> {
    pub async fn realize_append(&self, plan: &AppendPlan, state: &ItemState) -> Result<ContentRef> {
        let snapshot = state
            .bodies
            .values()
            .find(|b| {
                b.value.display == plan.previous
                    && b.value.text_part == Some(plan.part)
                    && b.value.text_bytes == plan.offset
            })
            .ok_or_else(|| MergeError::InvalidInput("append snapshot is absent".into()))?;
        let mut display = self.content.display(&plan.previous).await?;
        let selection =
            display.append.clone().ok_or_else(|| MergeError::InvalidInput("display has no append target".into()))?;
        let mut path = Vec::new();
        let mut overflow = None;
        let mut part =
            if let Some(part) = usize::try_from(selection.index.0).ok().and_then(|index| display.parts.get(index)) {
                part.clone()
            } else {
                let more = display
                    .more
                    .as_ref()
                    .ok_or_else(|| MergeError::InvalidInput("append target has no overflow".into()))?;
                let mut root = more.root.clone();
                let mut expected = None;
                let mut visited = std::collections::BTreeSet::new();
                loop {
                    if path.len() >= body_tree::MAX_DEPTH || !visited.insert(root.clone()) {
                        return Err(MergeError::InvalidInput("display parts cycle or depth exceeded".into()));
                    }
                    let page = self.content.page(root.clone()).await?;
                    let (start, end, entries) = part_extent(&page, &self.content).await?;
                    if let Some(child) = &expected {
                        verify_part_child(child, start, end, entries)?;
                    } else if start != more.start.0 || entries != more.count.0 {
                        return Err(MergeError::InvalidInput("overflow root coverage differs".into()));
                    }
                    match page {
                        ManifestPage::Metadata { codec: MetadataCodec::DisplayPartsV1, .. } => {
                            let bytes = self
                                .content
                                .metadata_bytes(&ContentRef::Manifest { root }, MetadataCodec::DisplayPartsV1)
                                .await?;
                            let leaf = ItemDisplayPartsV1::decode(&bytes).map_err(MergeError::InvalidInput)?;
                            let index =
                                usize::try_from(selection.index.0.checked_sub(leaf.start.0).ok_or_else(|| {
                                    MergeError::InvalidInput("append part precedes overflow leaf".into())
                                })?)
                                .map_err(|_| MergeError::InvalidInput("part index overflow".into()))?;
                            let part = leaf
                                .parts
                                .get(index)
                                .ok_or_else(|| MergeError::InvalidInput("overflow leaf misses append part".into()))?
                                .clone();
                            overflow = Some((leaf, index));
                            break part;
                        }
                        ManifestPage::Branch { children } => {
                            let key = format!("{:016x}", selection.index.0);
                            let index = children
                                .iter()
                                .position(|c| c.first_key <= key && key <= c.last_key)
                                .ok_or_else(|| MergeError::InvalidInput("overflow branch misses append part".into()))?;
                            let child = children[index].clone();
                            root = child.root.clone();
                            expected = Some(child);
                            path.push((children, index));
                        }
                        _ => return Err(MergeError::InvalidInput("invalid display parts tree page".into())),
                    }
                }
            };
        display.validate_append(&snapshot.value, Some(&part)).map_err(MergeError::InvalidInput)?;
        let mut before = BodyExtent { content: part.content.clone(), bytes: part.bytes };
        if before.bytes.0 > 0 {
            body_tree::locate(&self.content, &before, ByteCount(0)).await?;
        }
        let chunk = BodyExtent { content: plan.chunk.clone(), bytes: plan.bytes };
        if plan.bytes.0 == 0
            && let ContentRef::Manifest { root } = &plan.chunk
        {
            match self.content.page(root.clone()).await? {
                ManifestPage::Blocks { offset: ByteCount(0), blocks } if blocks.is_empty() => {}
                _ => {
                    return Err(MergeError::InvalidInput(
                        "zero-length chunk manifest has a nonempty or invalid extent".into(),
                    ));
                }
            }
        }

        let mut at = 0u64;
        let mut incomplete = Vec::new();
        while at < plan.bytes.0 {
            let (start, block) = body_tree::locate(&self.content, &chunk, ByteCount(at)).await?;
            if start.0 != at {
                return Err(MergeError::InvalidInput("append chunk block extent differs".into()));
            }
            let bytes = self.content.read_block(block.clone()).await?;
            validate_text_chunk(&mut incomplete, &bytes)?;
            before = body_tree::append_block(&self.content, Some(&before), block.clone()).await?;
            at = at.checked_add(u64::from(block.decoded_bytes)).ok_or(MergeError::CounterExhausted)?;
        }
        if at != plan.bytes.0 || !incomplete.is_empty() {
            return Err(MergeError::InvalidInput("append chunk length or UTF-8 boundary differs".into()));
        }
        part.content = before.content.clone();
        part.bytes = before.bytes;
        if overflow.is_none() {
            let other_preview = display
                .parts
                .iter()
                .filter(|p| p.index != part.index)
                .map(|p| p.preview.as_ref().map_or(0, String::len))
                .sum::<usize>();
            let available = DISPLAY_PREVIEW_BYTES.saturating_sub(other_preview);
            let take = usize::try_from(part.bytes.0.min(available as u64)).map_err(|_| MergeError::CounterExhausted)?;
            let bytes = body_tree::read_range(&self.content, &before, ByteCount(0), take, available).await?;
            let prefix = match std::str::from_utf8(&bytes) {
                Ok(text) => text,
                Err(error) if error.error_len().is_none() => std::str::from_utf8(&bytes[..error.valid_up_to()])
                    .map_err(|e| MergeError::InvalidInput(e.to_string()))?,
                Err(_) => return Err(MergeError::InvalidInput("body preview is not UTF-8".into())),
            };
            part.preview = Some(prefix.to_owned());
            let index = usize::try_from(selection.index.0).map_err(|_| MergeError::CounterExhausted)?;
            display.parts[index] = part;
        } else if let Some((mut leaf, index)) = overflow {
            leaf.parts[index] = part;
            let content = self.content.metadata(
                MetadataCodec::DisplayPartsV1,
                leaf.encode().map_err(MergeError::InvalidInput)?,
                leaf.owned_content(),
            )?;
            let ContentRef::Manifest { mut root } = content else {
                return Err(MergeError::InvalidInput("overflow metadata root missing".into()));
            };
            while let Some((mut children, index)) = path.pop() {
                children[index].root = root;
                root = self.content.stage_page(ManifestPage::Branch { children })?;
            }
            display.more.as_mut().ok_or_else(|| MergeError::InvalidInput("overflow root missing".into()))?.root = root;
        }
        self.content.metadata(
            MetadataCodec::DisplayV1,
            display.encode().map_err(MergeError::InvalidInput)?,
            display.owned_content(),
        )
    }
}
async fn part_extent(page: &ManifestPage, content: &super::content::Content<'_>) -> Result<(u64, u64, u64)> {
    match page {
        ManifestPage::Metadata { codec: MetadataCodec::DisplayPartsV1, payload, .. } => {
            let bytes = content.read_block(payload.clone()).await?;
            let leaf = ItemDisplayPartsV1::decode(&bytes).map_err(MergeError::InvalidInput)?;
            let end = leaf.start.0.checked_add(leaf.parts.len() as u64).ok_or(MergeError::CounterExhausted)?;
            Ok((leaf.start.0, end - 1, leaf.parts.len() as u64))
        }
        ManifestPage::Branch { children } => {
            if children.is_empty() || children.len() > 256 {
                return Err(MergeError::InvalidInput("invalid display parts fanout".into()));
            }
            let mut start = None;
            let mut end = None;
            let mut count = 0u64;
            for child in children {
                let (first, last) = part_range(child)?;
                if end.is_some_and(|e: u64| e.checked_add(1) != Some(first)) {
                    return Err(MergeError::InvalidInput("display parts gap".into()));
                }
                start.get_or_insert(first);
                end = Some(last);
                count = count.checked_add(child.entries.0).ok_or(MergeError::CounterExhausted)?;
            }
            Ok((
                start.ok_or_else(|| MergeError::InvalidInput("empty display parts".into()))?,
                end.ok_or_else(|| MergeError::InvalidInput("empty display parts".into()))?,
                count,
            ))
        }
        _ => Err(MergeError::InvalidInput("non-descriptor overflow page".into())),
    }
}
fn part_range(child: &crate::manifest::ManifestChild) -> Result<(u64, u64)> {
    let start = u64::from_str_radix(&child.first_key, 16).map_err(|e| MergeError::InvalidInput(e.to_string()))?;
    let end = u64::from_str_radix(&child.last_key, 16).map_err(|e| MergeError::InvalidInput(e.to_string()))?;
    if format!("{start:016x}") != child.first_key
        || format!("{end:016x}") != child.last_key
        || end.checked_sub(start).and_then(|n| n.checked_add(1)) != Some(child.entries.0)
    {
        return Err(MergeError::InvalidInput("display parts range or count differs".into()));
    }
    Ok((start, end))
}
fn verify_part_child(child: &crate::manifest::ManifestChild, start: u64, end: u64, entries: u64) -> Result<()> {
    if part_range(child)? != (start, end) || child.entries.0 != entries {
        return Err(MergeError::InvalidInput("display parts child binding differs".into()));
    }
    Ok(())
}

fn validate_text_chunk(tail: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    let mut start = 0;
    if let Some(first) = tail.first() {
        let width = if *first < 0xe0 {
            2
        } else if *first < 0xf0 {
            3
        } else {
            4
        };
        let take = (width - tail.len()).min(bytes.len());
        tail.extend_from_slice(&bytes[..take]);
        start = take;
        if tail.len() < width {
            return Ok(());
        }
        std::str::from_utf8(tail).map_err(|_| MergeError::InvalidInput("append chunk is not UTF-8".into()))?;
        tail.clear();
    }
    match std::str::from_utf8(&bytes[start..]) {
        Ok(_) => Ok(()),
        Err(error) if error.error_len().is_none() => {
            tail.extend_from_slice(&bytes[start + error.valid_up_to()..]);
            Ok(())
        }
        Err(_) => Err(MergeError::InvalidInput("append chunk is not UTF-8".into())),
    }
}
