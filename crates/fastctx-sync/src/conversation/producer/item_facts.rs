use super::{api::*, state::Engine};
use crate::conversation::*;
use fastctx_types::sync::conversation_timeline::*;

impl Engine<'_> {
    pub(super) async fn reduce_observed(
        &mut self,
        input: &MergeInput,
        trigger: &Evidence,
        observation: &Observation,
        mut causes: Vec<crate::catalog::RecordRef>,
    ) -> Result<Vec<Dependency>> {
        let evidence = &input.event.evidence;
        Ok({
            if observation.identity.conversation() != &self.conversation {
                return self.reduced(evidence, Err(Error::WrongConversation));
            }
            let old = self.item(&observation.identity.id()).await?;
            if old.is_none() {
                self.unique(&format!("creation/{:020}", observation.created.0), &observation.identity.id(), evidence)
                    .await?;
            }
            let mut placement = old.as_ref().and_then(|s| s.placement.clone());
            if let Some(value) = &observation.placement {
                self.reduced(
                    evidence,
                    crate::conversation::placement::choose(
                        &mut placement,
                        Versioned { evidence: evidence.clone(), value: value.clone() },
                    ),
                )?;
            }
            let neighbors = self.neighbors(placement.as_ref()).await?;
            causes.extend(neighbors.causes());
            let reduction =
                self.reduced(evidence, observe(old.as_ref(), observation, evidence, neighbors.as_refs()))?;
            self.placement_waiting(&reduction.state.id(), &reduction.effects.pending).await?;
            causes.extend(reduction.state.causes());
            Box::pin(self.save_item(old.as_ref(), &reduction.state, trigger)).await?;
            Box::pin(self.publish_effects(reduction.effects, trigger, causes)).await?
        })
    }
    pub(super) async fn reduce_placement(
        &mut self,
        input: &MergeInput,
        trigger: &Evidence,
        item: &crate::identity::ObjectId,
        placement: &Placement,
        mut causes: Vec<crate::catalog::RecordRef>,
    ) -> Result<Vec<Dependency>> {
        let evidence = &input.event.evidence;
        Ok({
            let Some(old) = self.item(item).await? else {
                return Ok(vec![Dependency::Item(item.clone())]);
            };
            let mut selected = old.placement.clone();
            self.reduced(
                evidence,
                crate::conversation::placement::choose(
                    &mut selected,
                    Versioned { evidence: evidence.clone(), value: placement.clone() },
                ),
            )?;
            let neighbors = self.neighbors(selected.as_ref()).await?;
            causes.extend(neighbors.causes());
            let reduction = self.reduced(evidence, place(&old, placement, evidence, neighbors.as_refs()))?;
            self.placement_waiting(&reduction.state.id(), &reduction.effects.pending).await?;
            causes.extend(reduction.state.causes());
            Box::pin(self.save_item(Some(&old), &reduction.state, trigger)).await?;
            Box::pin(self.publish_effects(reduction.effects, trigger, causes)).await?
        })
    }
    pub(super) async fn reduce_run(
        &mut self,
        input: &MergeInput,
        trigger: &Evidence,
        item: &crate::identity::ObjectId,
        link: &RunLink,
        mut causes: Vec<crate::catalog::RecordRef>,
    ) -> Result<Vec<Dependency>> {
        let evidence = &input.event.evidence;
        Ok({
            let Some(old) = self.item(item).await? else {
                return Ok(vec![Dependency::Item(item.clone())]);
            };
            let reduction = self.reduced(evidence, run(&old, link, evidence))?;
            causes.extend(reduction.state.causes());
            Box::pin(self.save_item(Some(&old), &reduction.state, trigger)).await?;
            Box::pin(self.publish_effects(reduction.effects, trigger, causes)).await?
        })
    }
    pub(super) async fn reduce_text_append(
        &mut self,
        input: &MergeInput,
        trigger: &Evidence,
        input_append: &TextAppend,
        mut causes: Vec<crate::catalog::RecordRef>,
    ) -> Result<Vec<Dependency>> {
        let evidence = &input.event.evidence;
        Ok({
            let Some(old) = self.item(&input_append.item).await? else {
                return Ok(vec![Dependency::Item(input_append.item.clone())]);
            };
            match self.reduced(evidence, append(&old, input_append, evidence))? {
                AppendOutcome::Ready(plan) => {
                    let display = Box::pin(self.realize_append(&plan, &old)).await?;
                    let reduction = self.reduced(evidence, plan.finish(&old, display))?;
                    causes.extend(reduction.state.causes());
                    Box::pin(self.save_item(Some(&old), &reduction.state, trigger)).await?;
                    Box::pin(self.publish_effects(reduction.effects, trigger, causes)).await?
                }
                AppendOutcome::Pending(dependency) => vec![dependency],
                AppendOutcome::Superseded | AppendOutcome::Replayed => Vec::new(),
                AppendOutcome::Redirect(_) => return self.reduced(evidence, Err(Error::InvalidAlias)),
            }
        })
    }
}
