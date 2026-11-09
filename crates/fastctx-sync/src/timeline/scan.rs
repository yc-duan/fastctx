//! Bounded immutable item scans for rebuilding a local position index. Unlike
//! a reader response, this has no wire envelope or duplicated position anchors.
use super::{
    TimelineItem, TimelinePosition,
    pages::{self, Budget, Meter, Outcome, PageSource, Work},
    position,
    tree::{self, Kind, TreeNode},
    update::Projection,
};
use crate::{
    manifest::{ManifestChild, ManifestRoot},
    store::Error,
};
use fastctx_types::sync::metadata::TimelineRoot;

pub struct IndexPage {
    pub items: Vec<TimelineItem>,
    /// False can include an exact full final page; the next strict seek then
    /// returns an empty completed page without guessing about missing data.
    pub done: bool,
}
/// Scans strictly after `after` in an authorized, admitted view of the expected
/// installed snapshot. Page/decode work obeys `budget`; rows obey `max_items`.
/// Missing dependencies are Pending, never an exhausted index. This retains no
/// root and grants no content access; the executor owns the actual root hold.
pub fn index_page(
    source: &impl PageSource,
    root: &ManifestRoot,
    expected: &Projection,
    after: Option<&TimelinePosition>,
    max_items: u32,
    budget: Budget,
) -> Result<Outcome<IndexPage>, Error> {
    if max_items == 0 || max_items > 256 {
        return Err(Error::Invalid("index scans require 1 to 256 items".into()));
    }
    pages::outcome((|| {
        let mut meter = Meter::new(budget);
        let root = tree::snapshot(source, root, &mut meter)?;
        if root.conversation != expected.conversation
            || root.projection != expected.revision
            || root.through.scope != expected.through.scope
            || root.through.stream != expected.through.stream
            || root.through.epoch != expected.through.epoch
            || root.through.committed > expected.through.committed
        {
            return Err(pages::corrupt("index scan root differs from its projection").into());
        }
        let anchor = after.map(position::key).transpose()?;
        let mut items = Vec::new();
        let done = match &root.items {
            Some(child) => {
                walk(source, child, &root, anchor.as_deref(), max_items as usize, &mut meter, &mut items, 0)?
            }
            None => true,
        };
        Ok(IndexPage { items, done })
    })())
}
#[allow(clippy::too_many_arguments)]
fn walk(
    source: &impl PageSource,
    child: &ManifestChild,
    root: &TimelineRoot,
    after: Option<&str>,
    max: usize,
    meter: &mut Meter,
    items: &mut Vec<TimelineItem>,
    depth: usize,
) -> Work<bool> {
    if depth >= tree::MAX_DEPTH {
        return Err(pages::corrupt("timeline tree exceeds its traversal depth").into());
    }
    if after.is_some_and(|after| child.last_key.as_str() <= after) {
        return Ok(true);
    }
    match tree::node(source, child, Kind::Items, root, meter)? {
        TreeNode::Branch(children) => {
            for child in children {
                if !walk(source, &child, root, after, max, meter, items, depth + 1)? {
                    return Ok(false);
                }
            }
        }
        TreeNode::Leaf(entries) => {
            for entry in entries {
                let key = entry.key()?;
                if after.is_some_and(|after| key.as_str() <= after) {
                    continue;
                }
                let tree::Entry::Item(entry, _) = entry else {
                    return Err(pages::corrupt("lookup in item scan").into());
                };
                items.push(entry.item);
                if items.len() == max {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}
