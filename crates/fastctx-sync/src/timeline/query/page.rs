//! Count each encoded item once. Array separators and the two boundary anchors
//! are additive; no growing page is reserialized after every appended row.
use super::{
    Error, PAGE_BYTES, QueryPlan, TimelineAnchor, TimelineItem, TimelinePage, TimelineResult, backward, pages,
};
use crate::identity::ObjectId;
use fastctx_types::sync::{timeline::TimelinePosition, transport_codec::bounded_json};
use std::collections::BTreeSet;

pub(super) struct Builder<'a> {
    plan: &'a QueryPlan,
    items: Vec<TimelineItem>,
    ids: BTreeSet<ObjectId>,
    last: Option<TimelinePosition>,
    base_bytes: usize,
    items_bytes: usize,
    first_position_bytes: Option<usize>,
}
impl<'a> Builder<'a> {
    pub fn new(plan: &'a QueryPlan) -> Result<Self, Error> {
        let empty = TimelineResult::Page {
            page: Box::new(TimelinePage { fence: plan.fence.clone(), items: Vec::new(), older: None, newer: None }),
        };
        let base_bytes = bounded_json(&empty, plan.limit.bytes as usize).map_err(|_| Error::Busy)?.len();
        Ok(Self {
            plan,
            items: Vec::new(),
            ids: BTreeSet::new(),
            last: None,
            base_bytes,
            items_bytes: 0,
            first_position_bytes: None,
        })
    }
    pub fn push(&mut self, item: TimelineItem) -> Result<bool, Error> {
        if self.items.len() >= self.plan.limit.items as usize {
            return Ok(false);
        }
        item.display.validate().map_err(Error::Invalid)?;
        item.display.validate_scopes(&self.plan.fence.conversation.scope).map_err(Error::Invalid)?;
        item.body.validate().map_err(Error::Invalid)?;
        if item.id != item.position.item
            || item.revision.0 == 0
            || item.position.display_ancestry.is_empty()
            || item.body.id().domain != self.plan.fence.conversation.scope
            || !self.ids.insert(item.id.clone())
            || self
                .last
                .as_ref()
                .is_some_and(|p| if backward(&self.plan.anchor) { p <= &item.position } else { p >= &item.position })
            || match &self.plan.anchor {
                TimelineAnchor::Before(p) => &item.position >= p,
                TimelineAnchor::After(p) => &item.position <= p,
                TimelineAnchor::Latest => false,
            }
        {
            return Err(pages::corrupt("query rows violate the fence's ordering or scope"));
        }
        let item_bytes = bounded_json(&item, PAGE_BYTES as usize).map_err(|_| Error::Busy)?.len();
        let position_bytes = bounded_json(&item.position, PAGE_BYTES as usize).map_err(|_| Error::Busy)?.len();
        let first = self.first_position_bytes.unwrap_or(position_bytes);
        let items_bytes = self
            .items_bytes
            .checked_add(item_bytes)
            .and_then(|n| n.checked_add(usize::from(!self.items.is_empty())))
            .ok_or(Error::Busy)?;
        // Replace the empty envelope's two four-byte null anchors with positions.
        let total = self
            .base_bytes
            .checked_add(items_bytes)
            .and_then(|n| n.checked_add(first))
            .and_then(|n| n.checked_add(position_bytes))
            .and_then(|n| n.checked_sub(8))
            .ok_or(Error::Busy)?;
        if total > self.plan.limit.bytes as usize {
            if self.items.is_empty() {
                return Err(Error::Busy);
            }
            return Ok(false);
        }
        self.first_position_bytes = Some(first);
        self.items_bytes = items_bytes;
        self.last = Some(item.position.clone());
        self.items.push(item);
        Ok(true)
    }
    pub fn finish(self) -> TimelinePage {
        let mut items = self.items;
        if backward(&self.plan.anchor) {
            items.reverse()
        }
        TimelinePage {
            fence: self.plan.fence.clone(),
            older: items.first().map(|v| v.position.clone()),
            newer: items.last().map(|v| v.position.clone()),
            items,
        }
    }
}
