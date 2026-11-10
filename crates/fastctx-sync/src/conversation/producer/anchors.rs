//! Reverse gap anchors share the producer's durable, paged relation repairs.
use super::{
    api::*,
    jobs::JobKind,
    state::{Engine, NeighborRows},
};
use crate::conversation::*;
use fastctx_types::sync::conversation_timeline::*;

fn anchor(state: &ItemState) -> Option<&crate::identity::ObjectId> {
    state.placement.as_ref()?.value.order.as_ref()?.anchor()
}

impl Engine<'_> {
    pub(super) async fn placement_waiting(
        &mut self,
        item: &crate::identity::ObjectId,
        pending: &[Dependency],
    ) -> Result<()> {
        let key = format!("unresolved-placement/{}", item.as_str());
        if pending.is_empty() {
            self.delete(&key).await?;
        } else {
            let dependencies: Vec<_> = pending.iter().map(super::keys::dependency).collect();
            self.put(&key, &dependencies).await?;
        }
        Ok(())
    }

    pub(super) async fn anchor_neighbors(&mut self, placed: &Placement, result: &mut NeighborRows) -> Result<()> {
        let Some(id) = placed.order.as_ref().and_then(DisplayOrder::anchor) else {
            return Ok(());
        };
        result.anchor = self.item(id).await?;
        if let Some(target) = result.anchor.as_ref().and_then(|s| s.alias.as_ref()) {
            result.resolved_anchor = self.item(target).await?;
            if let Some(alias) = self.get::<MergeInput>(&format!("alias/{}", id.as_str())).await? {
                result.aliases.push(alias.event.evidence.record);
                if let TimelineFact::Alias { receipt, .. } = alias.event.fact {
                    result.aliases.push(receipt);
                }
            }
        }
        Ok(())
    }

    pub(super) async fn save_anchor(
        &mut self,
        old: Option<&ItemState>,
        state: &ItemState,
        trigger: &Evidence,
    ) -> Result<()> {
        if state.alias.is_some() {
            self.placement_waiting(&state.id(), &[]).await?;
        }
        let next = match anchor(state).filter(|_| state.alias.is_none()) {
            Some(id) => Some(self.effective(id).await?),
            None => None,
        };
        if let Some(previous) = old.and_then(anchor) {
            let prior = [previous.clone(), self.effective(previous).await?];
            for id in prior {
                if Some(&id) != next.as_ref() {
                    self.delete(&format!("reverse/anchor/{}/{}", id.as_str(), state.id().as_str())).await?;
                }
            }
        }
        if let Some(id) = next {
            self.put(&format!("reverse/anchor/{}/{}", id.as_str(), state.id().as_str()), &state.id()).await?;
        }
        if old.and_then(|s| s.position.as_ref()) != state.position.as_ref()
            || old.and_then(|s| s.placement.as_ref()) != state.placement.as_ref()
            || old.and_then(|s| s.alias.as_ref()) != state.alias.as_ref()
        {
            self.wake(&Dependency::Anchor(state.id()), trigger).await?;
            if self.has(&format!("reverse/anchor/{}/", state.id().as_str())).await? {
                self.enqueue(JobKind::Anchors { anchor: state.id() }, trigger).await?;
            }
        }
        Ok(())
    }
}
