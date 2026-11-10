//! Read planning over a verified fence. The executor owns permissions, root-use
//! records, SQL snapshots and scheduling; this module never opens those services.
use super::{
    TimelineAnchor, TimelineFence, TimelineItem, TimelinePage, TimelineResult,
    pages::{self, Budget, Meter, Outcome, PageSource, Work},
    position,
    tree::{self, Entry, Kind, TreeNode},
};
use crate::{
    cursor::Revision,
    identity::{ObjectId, ObjectKey},
    manifest::{ManifestChild, ManifestRoot},
    store::{Error, PageLimit},
};
use fastctx_types::sync::{PAGE_BYTES, metadata::TimelineRoot};
mod page;
use page::Builder;

#[derive(Clone, Debug)]
pub struct Head {
    pub conversation: ObjectKey,
    pub root: ManifestRoot,
    pub projection: Revision,
    pub indexed_projection: Option<Revision>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    CurrentSql,
    Immutable,
}
#[derive(Clone, Debug)]
pub struct QueryPlan {
    pub route: Route,
    pub fence: TimelineFence,
    pub anchor: TimelineAnchor,
    pub limit: PageLimit,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseState {
    Active,
    Expired,
}
#[derive(Debug)]
pub enum ItemResult {
    Item(Option<Box<TimelineItem>>),
    ReanchorNeeded,
}
/// Head and fence come from one authenticated projection/lease snapshot. A
/// supplied client fence must be checked by the executor before calling here.
pub fn query_plan(
    head: &Head,
    fence: TimelineFence,
    anchor: TimelineAnchor,
    limit: PageLimit,
) -> Result<QueryPlan, Error> {
    limits(limit)?;
    if head.conversation != fence.conversation || fence.root.0.domain != fence.conversation.scope {
        return Err(Error::Invalid("timeline query changes conversation".into()));
    }
    let route = if head.root == fence.root
        && head.projection == fence.projection
        && head.indexed_projection == Some(head.projection)
    {
        Route::CurrentSql
    } else {
        Route::Immutable
    };
    Ok(QueryPlan { route, fence, anchor, limit })
}
#[derive(Debug)]
pub struct SqlSeek {
    pub statement: &'static str,
    pub position: Option<Vec<u8>>,
    pub limit: u32,
}
pub fn sql_seek(plan: &QueryPlan) -> Result<SqlSeek, Error> {
    limits(plan.limit)?;
    if plan.route != Route::CurrentSql {
        return Err(Error::Invalid("this fence requires its immutable tree".into()));
    }
    let (statement, anchor) = match &plan.anchor {
        TimelineAnchor::Latest => (
            "SELECT body FROM sync_timeline_items INDEXED BY sync_timeline_order WHERE hub=?1 AND scope=?2 AND conversation=?3 AND active=1 ORDER BY position DESC,item DESC LIMIT ?5",
            None,
        ),
        TimelineAnchor::Before(p) => (
            "SELECT body FROM sync_timeline_items INDEXED BY sync_timeline_order WHERE hub=?1 AND scope=?2 AND conversation=?3 AND active=1 AND position<?4 ORDER BY position DESC,item DESC LIMIT ?5",
            Some(position::encode(p)?),
        ),
        TimelineAnchor::After(p) => (
            "SELECT body FROM sync_timeline_items INDEXED BY sync_timeline_order WHERE hub=?1 AND scope=?2 AND conversation=?3 AND active=1 AND position>?4 ORDER BY position,item LIMIT ?5",
            Some(position::encode(p)?),
        ),
    };
    Ok(SqlSeek { statement, position: anchor, limit: plan.limit.items + 1 })
}
/// Rows are the exact sql_seek result, decoded from the bounded TimelineItem
/// projection, in the same read snapshot as the head's index revision check.
pub fn page_from_rows(
    plan: &QueryPlan,
    rows: impl Iterator<Item = Result<TimelineItem, Error>>,
) -> Result<TimelinePage, Error> {
    limits(plan.limit)?;
    if plan.route != Route::CurrentSql {
        return Err(Error::Invalid("SQL rows cannot answer an old or rebuilding fence".into()));
    }
    let mut builder = Builder::new(plan)?;
    for row in rows.take(plan.limit.items as usize + 1) {
        if !builder.push(row?)? {
            break;
        }
    }
    Ok(builder.finish())
}
/// The executor checks current object access and the persisted lease. Holding a
/// root does not grant reads; it rechecks revocation before exposing the result.
/// Missing pages remain Pending even if the referenced body is not local yet.
pub fn page(
    source: &impl PageSource,
    plan: &QueryPlan,
    lease: LeaseState,
    budget: Budget,
) -> Result<Outcome<TimelineResult>, Error> {
    if lease == LeaseState::Expired {
        return Ok(Outcome::Ready(TimelineResult::ReanchorNeeded));
    }
    pages::outcome(read_page(source, plan, budget))
}
fn read_page(source: &impl PageSource, plan: &QueryPlan, budget: Budget) -> Work<TimelineResult> {
    limits(plan.limit)?;
    let mut meter = Meter::new(budget);
    let root = read_root(source, &plan.fence, &mut meter)?;
    let anchor = match &plan.anchor {
        TimelineAnchor::Latest => None,
        TimelineAnchor::Before(p) | TimelineAnchor::After(p) => Some(position::key(p)?),
    };
    let mut builder = Builder::new(plan)?;
    if let Some(child) = &root.items {
        walk(source, child, &root, anchor.as_deref(), backward(&plan.anchor), &mut meter, &mut builder, 0)?;
    }
    Ok(TimelineResult::Page { page: Box::new(builder.finish()) })
}
/// Looks up an ID within this immutable fence, then verifies the ordered entry.
/// Only a completed lookup may return None; missing metadata is Pending.
pub fn item(
    source: &impl PageSource,
    fence: &TimelineFence,
    id: &ObjectId,
    lease: LeaseState,
    budget: Budget,
) -> Result<Outcome<ItemResult>, Error> {
    if lease == LeaseState::Expired {
        return Ok(Outcome::Ready(ItemResult::ReanchorNeeded));
    }
    pages::outcome((|| {
        let mut meter = Meter::new(budget);
        let root = read_root(source, fence, &mut meter)?;
        let value = tree::item(source, &root, id, &mut meter)?;
        Ok(ItemResult::Item(match value {
            Some(Entry::Item(entry, _)) => Some(Box::new(entry.item)),
            None => None,
            _ => unreachable!(),
        }))
    })())
}
/// The projection revision names the snapshot. Records installed after it that
/// changed no item move the head's cursor and leave the root as it was, so a
/// fence's cursor may be at or past its root's, on the same stream and epoch.
pub(super) fn read_root(source: &impl PageSource, fence: &TimelineFence, meter: &mut Meter) -> Work<TimelineRoot> {
    let root = tree::snapshot(source, &fence.root, meter)?;
    let at = fence.through.cursor();
    if root.conversation != fence.conversation
        || root.projection != fence.projection
        || root.through.scope != at.scope
        || root.through.stream != at.stream
        || root.through.epoch != at.epoch
        || root.through.committed > at.committed
    {
        return Err(pages::corrupt("timeline root differs from its fence").into());
    }
    Ok(root)
}
#[allow(clippy::too_many_arguments)]
fn walk(
    source: &impl PageSource,
    child: &ManifestChild,
    root: &TimelineRoot,
    anchor: Option<&str>,
    reverse: bool,
    meter: &mut Meter,
    builder: &mut Builder<'_>,
    depth: usize,
) -> Work<bool> {
    if depth >= tree::MAX_DEPTH {
        return Err(pages::corrupt("timeline tree exceeds its traversal depth").into());
    }
    if anchor.is_some_and(|key| if reverse { child.first_key.as_str() >= key } else { child.last_key.as_str() <= key })
    {
        return Ok(true);
    }
    match tree::node(source, child, Kind::Items, root, meter)? {
        TreeNode::Branch(mut children) => {
            if reverse {
                children.reverse()
            }
            for child in children {
                if !walk(source, &child, root, anchor, reverse, meter, builder, depth + 1)? {
                    return Ok(false);
                }
            }
        }
        TreeNode::Leaf(mut entries) => {
            if reverse {
                entries.reverse()
            }
            for entry in entries {
                let key = entry.key()?;
                if anchor.is_some_and(|a| if reverse { key.as_str() >= a } else { key.as_str() <= a }) {
                    continue;
                }
                let Entry::Item(entry, _) = entry else { return Err(pages::corrupt("lookup in ordered tree").into()) };
                if !builder.push(entry.item)? {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

fn backward(anchor: &TimelineAnchor) -> bool {
    !matches!(anchor, TimelineAnchor::After(_))
}
pub(super) fn limits(limit: PageLimit) -> Result<(), Error> {
    if limit.items == 0 || limit.items > 100 || limit.bytes == 0 || limit.bytes > PAGE_BYTES {
        return Err(Error::Invalid("timeline read requires 1..100 items and at most 256 KiB".into()));
    }
    Ok(())
}
