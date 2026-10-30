//! Coalesced plan projections. Account invalidations, not periodic context
//! refreshes, make an unchanged plan worth reading from the hub again.

use crate::Node;
use fastctx_types::{
    Address, ResidentId,
    access::{HubAccount, HubId},
    account_records::Changed,
    residents::Origin,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub(crate) struct Reads(Mutex<HashMap<ResidentId, (HubAccount, Address, u64, bool)>>);
impl Reads {
    pub(crate) fn begin(&self, id: &ResidentId, scope: &HubAccount, owner: &Address) -> Option<u64> {
        let mut reads = self.0.lock().expect("never poisoned");
        let entry = reads.entry(id.clone()).or_insert_with(|| (scope.clone(), owner.clone(), 0, false));
        if entry.0 != *scope || entry.1 != *owner {
            *entry = (scope.clone(), owner.clone(), entry.2.wrapping_add(1), false);
        }
        (!entry.3).then_some(entry.2)
    }
    pub(crate) fn done(&self, id: &ResidentId, generation: u64) {
        if let Some(entry) = self.0.lock().expect("never poisoned").get_mut(id)
            && entry.2 == generation
        {
            entry.3 = true;
        }
    }
    pub(crate) fn invalidate(&self, id: &ResidentId) {
        if let Some(entry) = self.0.lock().expect("never poisoned").get_mut(id) {
            entry.2 = entry.2.wrapping_add(1);
            entry.3 = false;
        }
    }
    pub(crate) fn clear(&self) {
        self.0.lock().expect("never poisoned").clear();
    }
}

pub(crate) fn changed(node: &Arc<Node>, hub: &HubId, change: &Changed) {
    if change.metadata.kind != fastctx_types::plan::KIND && change.metadata.kind != fastctx_types::placement::KIND {
        return;
    }
    for resident in node.residents.everyone().filter(|resident| resident.removed.is_none()) {
        let owner = match &resident.about.origin {
            Origin::User => &resident.id,
            Origin::Colleague { asked_by, .. } => &asked_by.resident,
        };
        if owner.as_str() == change.metadata.key && node.serves(&resident.about.acting, hub, &change.account) {
            node.plan_reads.invalidate(&resident.id);
            if change.metadata.kind == fastctx_types::placement::KIND {
                node.locations.invalidate(&resident.id);
            }
            crate::context::request_plan(node, &resident.id);
        }
    }
}
