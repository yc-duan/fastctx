//! Each step decodes at most its page budget and changes a bounded frontier.
use super::{
    layout::{Child, Layout},
    model::{SupplyBudget, SupplyFailure, SupplyProgress, Work},
    protection, qualification,
    rows::{self, Check},
};
use crate::metrics::{Cause, Counter, Metrics, PlatformMetrics};
use crate::{
    blocks::{pages, rows as content},
    manifest::ContentRef,
    store::sql,
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

use std::sync::Arc;

struct StepBudget {
    nodes: usize,
    page_bytes: usize,
    cached: Option<(Child, Arc<Layout>)>,
}
enum Loaded {
    Page(Arc<Layout>),
    Missing,
    Budget,
}
enum Advance {
    Covered,
    Continue,
    Missing(ContentRef),
    Budget,
    Rejected,
}

pub(super) fn step(db: &Connection, check: &mut Check, budget: SupplyBudget) -> Result<SupplyProgress> {
    let mut budget = StepBudget { nodes: budget.nodes, page_bytes: budget.page_bytes, cached: None };
    while budget.nodes != 0 {
        let Some((at, mut work)) = rows::first_work(db, &check.ticket)? else {
            let progress = if content::complete(db, &check.request.root)? {
                SupplyProgress::Ready
            } else {
                SupplyProgress::Pending { missing: Some(check.request.root.clone()) }
            };
            let phase = if matches!(progress, SupplyProgress::Ready) { "ready" } else { "waiting" };
            rows::progress(db, check, phase, progress.clone())?;
            return Ok(progress);
        };
        budget.nodes -= 1;
        PlatformMetrics.add(Counter::SupplyNodes, Cause::Ordinary, 1);
        let result = if work.expand {
            expand(db, check, &mut work, &mut budget)
        } else {
            classify(db, check, &mut work, &mut budget)
        };
        let result = match result {
            Ok(value) => value,
            Err(error) if error.kind() == fastctx_storage::ErrorKind::Invalid => {
                let progress =
                    SupplyProgress::Rejected { node: work.node.root, reason: SupplyFailure::InvalidPosition };
                rows::progress(db, check, "rejected", progress.clone())?;
                return Ok(progress);
            }
            Err(error) => return Err(error),
        };
        match result {
            Advance::Covered => {
                if !work.expand {
                    PlatformMetrics.add(Counter::SupplyReused, Cause::Ordinary, 1);
                }
                rows::remove_work(db, &check.ticket, at)?;
            }
            Advance::Continue => rows::put_work(db, &check.ticket, at, &work)?,
            Advance::Missing(root) => {
                rows::put_work(db, &check.ticket, at, &work)?;
                let progress = SupplyProgress::Pending { missing: Some(root) };
                rows::progress(db, check, "walking", progress.clone())?;
                return Ok(progress);
            }
            Advance::Budget => {
                rows::put_work(db, &check.ticket, at, &work)?;
                break;
            }
            Advance::Rejected => {
                let progress = SupplyProgress::Rejected { node: work.node.root, reason: SupplyFailure::MissingSupply };
                rows::progress(db, check, "rejected", progress.clone())?;
                return Ok(progress);
            }
        }
    }
    let progress = SupplyProgress::Pending { missing: None };
    rows::progress(db, check, "walking", progress.clone())?;
    Ok(progress)
}

fn classify(db: &Connection, check: &Check, work: &mut Work, budget: &mut StepBudget) -> Result<Advance> {
    if matches!(work.node.root, ContentRef::Single { .. }) {
        work.node.address = Layout::single(&work.node.root, &work.node.address)?.address;
    }
    if work.bases.iter().any(|basis| basis.root == work.node.root && basis.address == work.node.address) {
        return Ok(Advance::Covered);
    }
    if protection::grant(db, check, &work.node.root)? {
        return Ok(Advance::Covered);
    }
    match qualification::supplied(db, check, &work.node.root)? {
        qualification::NodeSupply::Ready => {
            return match work.node.root {
                ContentRef::Single { .. } => Ok(Advance::Covered),
                ContentRef::Manifest { .. } => {
                    work.expand = true;
                    Ok(Advance::Continue)
                }
            };
        }
        qualification::NodeSupply::Pending => return Ok(Advance::Missing(work.node.root.clone())),
        qualification::NodeSupply::Absent => {}
    }
    if work.cursor >= work.bases.len() {
        let present = match &work.node.root {
            ContentRef::Single { .. } => content::complete(db, &work.node.root)?,
            ContentRef::Manifest { root } => db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_asset_manifests WHERE hub=?1 AND scope=?2 AND hash=?3 AND validated=1)",
                params![root.0.domain.hub.to_string(), root.0.domain.scope.as_str(), root.0.digest.as_str()], |row|row.get(0),
            )?,
        };
        return Ok(if present || work.depth == 0 {
            Advance::Rejected
        } else {
            Advance::Missing(work.node.root.clone())
        });
    }
    let basis = work.seek.clone().unwrap_or_else(|| work.bases[work.cursor].clone());
    match load(db, &basis, budget)? {
        Loaded::Missing => Ok(Advance::Missing(basis.root)),
        Loaded::Budget => Ok(Advance::Budget),
        Loaded::Page(layout) => {
            let target = work.node.address.clone();
            if basis.root == work.node.root
                && target.same_tree(&layout.address)
                && let Ok(resolved) = target.resolve(layout.address.span)
                && resolved == layout.address
            {
                return Ok(Advance::Covered);
            }
            if let Some(child) = layout.toward(&target)? {
                work.seek = Some(child);
            } else {
                work.cursor += 1;
                work.seek = None;
            }
            Ok(Advance::Continue)
        }
    }
}

fn expand(db: &Connection, check: &mut Check, work: &mut Work, budget: &mut StepBudget) -> Result<Advance> {
    let layout = match load(db, &work.node, budget)? {
        Loaded::Missing => return Ok(Advance::Missing(work.node.root.clone())),
        Loaded::Budget => return Ok(Advance::Budget),
        Loaded::Page(layout) => layout,
    };
    while budget.nodes != 0 {
        let Some(node) = layout.child(work.child)? else {
            return Ok(Advance::Covered);
        };
        budget.nodes -= 1;
        let child = Work {
            node,
            bases: work.bases.clone(),
            cursor: 0,
            seek: None,
            expand: false,
            child: 0,
            depth: work.depth.checked_add(1).ok_or_else(|| sql::invalid("supply depth overflow"))?,
        };
        rows::put_work(db, &check.ticket, check.next_work, &child)?;
        check.next_work =
            check.next_work.checked_add(1).ok_or_else(|| sql::invalid("supply work sequence overflow"))?;
        work.child += 1;
    }
    if work.child == layout.children.len() { Ok(Advance::Covered) } else { Ok(Advance::Continue) }
}

fn load(db: &Connection, node: &Child, budget: &mut StepBudget) -> Result<Loaded> {
    let ContentRef::Manifest { root } = &node.root else {
        return Ok(Loaded::Page(Arc::new(Layout::single(&node.root, &node.address)?)));
    };
    if let Some((key, layout)) = &budget.cached
        && key == node
    {
        return Ok(Loaded::Page(layout.clone()));
    }
    let id = &root.0;
    let state = db
        .query_row(
            "SELECT length(page),validated FROM sync_asset_manifests WHERE hub=?1 AND scope=?2 AND hash=?3",
            params![id.domain.hub.to_string(), id.domain.scope.as_str(), id.digest.as_str()],
            |row| Ok((row.get::<_, u32>(0)?, row.get::<_, bool>(1)?)),
        )
        .optional()?;
    let Some((bytes, true)) = state else {
        return Ok(Loaded::Missing);
    };
    let bytes = bytes as usize;
    if bytes > fastctx_types::sync::PAGE_BYTES as usize {
        return Err(sql::invalid("stored supply page exceeds its bound"));
    }
    if bytes > budget.page_bytes {
        return Ok(Loaded::Budget);
    }
    budget.page_bytes -= bytes;
    PlatformMetrics.add(Counter::SupplyPageBytes, Cause::Ordinary, bytes as u64);
    let page = pages::get(db, root)?;
    let layout = Arc::new(Layout::page(&page, &node.address)?);
    budget.cached = Some((node.clone(), layout.clone()));
    Ok(Loaded::Page(layout))
}
