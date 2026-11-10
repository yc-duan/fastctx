//! Keyset storage-block selection follows only an adopted ordered manifest tree.
mod page;
use super::*;
use crate::{
    cursor::{BlockRange, Count},
    manifest::IndexedBlock,
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
        let _permit =
            blocks.buffer_budget().try_acquire(4 * fastctx_types::sync::PAGE_BYTES as usize + PAGE_BLOCKS * 512)?;
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
            let page = page::decode(reply.page_json.as_bytes(), start, &missing, PAGE_BLOCKS - output.len())?;
            let count = page.count();
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
                page::Selected::Blocks { blocks, .. } => output.extend(blocks),
                page::Selected::Branch { children, .. } => {
                    let available = WALK_PAGES.saturating_sub(pages);
                    let mut first: Vec<_> =
                        children.into_iter().chain(pending.drain(..).rev()).take(available).collect();
                    first.reverse();
                    pending = first;
                }
            }
        }

        self.custody(peer, &holding, &reference, None).await?;
        loop {
            let after = output.last().map(|block| block.index.0 + 1).unwrap_or(0);
            let remaining = missing
                .iter()
                .filter_map(|range| {
                    let start = range.start.0.max(after);
                    (start < range.end.0).then_some(BlockRange { start: Count(start), end: range.end })
                })
                .collect();
            let answer =
                Message::Blocks { holding: holding.clone(), manifest: root.clone(), blocks: output, remaining };
            if read::size(&answer).map_err(|error| error.0)? <= fastctx_types::sync::PAGE_BYTES as usize {
                return Ok(answer);
            }
            let Message::Blocks { blocks, .. } = answer else { unreachable!() };
            output = blocks;
            if output.len() <= 1 {
                return Err(Error::Invalid("one block range result exceeds the metadata page bound".into()));
            }
            output.truncate(output.len() / 2);
        }
    }
}
