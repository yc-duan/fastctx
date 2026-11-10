//! Keyset storage-block selection follows only an adopted ordered manifest tree.
use super::*;
use crate::{
    cursor::{BlockRange, Count},
    manifest::{IndexedBlock, ManifestPage},
};
use fastctx_types::sync::session::MAX_RANGES;

const PAGE_BLOCKS: usize = 128;
const WALK_PAGES: usize = 128;
impl Service {
    pub(crate) async fn wanted(
        &self,
        peer: &Peer,
        holding: HoldingPeriod,
        root: ManifestRoot,
        missing: Vec<BlockRange>,
    ) -> Result<Message, Error> {
        if missing.is_empty()
            || missing.len() > MAX_RANGES
            || missing.iter().any(|range| range.validate().is_err())
            || missing.windows(2).any(|pair| pair[0].end > pair[1].start)
        {
            return Err(Error::Invalid("block ranges must be bounded, ordered and disjoint".into()));
        }
        let reference = ContentRef::Manifest { root: root.clone() };
        let custody = self.custody(peer, &holding, &reference, None).await?;
        let blocks = self.supply_blocks()?;
        blocks.require_adopted(&custody, &reference)?;
        let _permit = blocks.buffer_budget().try_acquire(2 * fastctx_types::sync::PAGE_BYTES as usize)?;
        let mut output = Vec::new();
        let mut pending = vec![(root.clone(), 0_u64, None)];
        let mut pages = 0;
        let mut total = None;
        while let Some((page_root, start, expected)) = pending.pop() {
            if output.len() == PAGE_BLOCKS {
                break;
            }
            pages += 1;
            if pages > WALK_PAGES {
                return Err(Error::Busy);
            }
            let reply = blocks
                .stored_page(&page_root)?
                .ok_or_else(|| Error::Unavailable("an adopted manifest page is not stored".into()))?;
            let prepared = crate::transport_flow::decode_json::<ManifestPage>(
                reply.page_json.as_bytes(),
                &blocks.buffer_budget(),
            )?;
            let page = prepared.value();
            let count = match page {
                ManifestPage::Blocks { blocks, .. } => blocks.len() as u64,
                ManifestPage::Branch { children } => children
                    .iter()
                    .try_fold(0_u64, |sum, child| sum.checked_add(child.entries.0))
                    .ok_or_else(|| Error::Corrupt("manifest block count overflow".into()))?,
                _ => return Err(Error::Invalid("block ranges require an ordered Blocks/Branch tree".into())),
            };
            if expected.is_some_and(|expected| expected != count) {
                return Err(Error::Corrupt("ordered manifest child count differs from its parent".into()));
            }
            let end =
                start.checked_add(count).ok_or_else(|| Error::Corrupt("manifest block ordinal overflow".into()))?;
            if total.is_none() {
                total = Some(end);
                if missing.last().is_some_and(|range| range.end.0 > end) {
                    return Err(Error::Invalid("requested block range exceeds its manifest".into()));
                }
            }
            match page {
                ManifestPage::Blocks { blocks, .. } => {
                    for (offset, content) in blocks.iter().enumerate() {
                        let index = start + offset as u64;
                        if missing.iter().any(|range| range.start.0 <= index && index < range.end.0) {
                            output.push(IndexedBlock { index: Count(index), content: content.clone() });
                            if output.len() == PAGE_BLOCKS {
                                break;
                            }
                        }
                    }
                }
                ManifestPage::Branch { children } => {
                    let mut starts = Vec::new();
                    let mut next = start;
                    for child in children {
                        let end = next
                            .checked_add(child.entries.0)
                            .ok_or_else(|| Error::Corrupt("manifest block ordinal overflow".into()))?;
                        if missing.iter().any(|range| range.start.0 < end && next < range.end.0) {
                            starts.push((child.root.clone(), next, Some(child.entries.0)));
                        }
                        next = end;
                    }
                    if pending.len() + starts.len() > WALK_PAGES {
                        return Err(Error::Busy);
                    }
                    pending.extend(starts.into_iter().rev());
                }
                _ => unreachable!("ordered variants checked above"),
            }
        }
        let after = output.last().map(|block| block.index.0 + 1).unwrap_or(0);
        let remaining = missing
            .into_iter()
            .filter_map(|range| {
                let start = range.start.0.max(after);
                (start < range.end.0).then_some(BlockRange { start: Count(start), end: range.end })
            })
            .collect();
        self.custody(peer, &holding, &reference, None).await?;
        let answer = Message::Blocks { holding, manifest: root, blocks: output, remaining };
        if read::size(&answer).map_err(|error| error.0)? > fastctx_types::sync::PAGE_BYTES as usize {
            return Err(Error::Busy);
        }
        Ok(answer)
    }
}
