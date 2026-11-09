use super::{api::*, state::Engine};
use crate::conversation::*;
use fastctx_types::sync::conversation_timeline::*;

impl Engine<'_> {
    pub async fn apply(&mut self, input: &MergeInput, trigger: &Evidence) -> Result<()> {
        let mut causes = input.causes.clone();
        causes.push(input.event.evidence.record.clone());
        let evidence = &input.event.evidence;
        let addressed = match &input.event.fact {
            TimelineFact::Observed { observation } => Some(observation.identity.id()),
            TimelineFact::TextAppend { append } => Some(append.item.clone()),
            TimelineFact::Placement { item, .. } | TimelineFact::Run { item, .. } => Some(item.clone()),
            TimelineFact::Image { picture } => Some(picture.item.clone()),
            TimelineFact::Branch { change: BranchChange::Membership { item, .. } } => Some(item.clone()),
            _ => None,
        };
        let mut fact = input.event.fact.clone();
        if let Some(id) = addressed
            && let Some(source) = self.item(&id).await?
            && let Some(to) = &source.alias
        {
            let Some(target) = self.item(to).await? else {
                return self.set_waiting(input, vec![Dependency::Item(to.clone())]).await;
            };
            fact = self.reduced(evidence, redirect_fact(&fact, &source, &target))?;
            if let Some(alias) = self.get::<MergeInput>(&format!("alias/{}", source.id().as_str())).await? {
                causes.push(alias.event.evidence.record);
            }
        }
        let pending = match &fact {
            TimelineFact::Observed { observation } => {
                Box::pin(self.reduce_observed(input, trigger, observation, causes)).await?
            }
            TimelineFact::Turn { turn: declaration } => {
                Box::pin(self.reduce_turn(input, trigger, declaration, causes)).await?
            }
            TimelineFact::Placement { item, placement } => {
                Box::pin(self.reduce_placement(input, trigger, item, placement, causes)).await?
            }
            TimelineFact::Run { item, run: link } => {
                Box::pin(self.reduce_run(input, trigger, item, link, causes)).await?
            }
            TimelineFact::TextAppend { append: input_append } => {
                Box::pin(self.reduce_text_append(input, trigger, input_append, causes)).await?
            }
            TimelineFact::Image { picture: link } => Box::pin(self.reduce_image(input, trigger, link, causes)).await?,
            TimelineFact::Agent { agent: link } => Box::pin(self.reduce_agent(input, trigger, link, causes)).await?,
            TimelineFact::HostRewritten { rewrite: change } => {
                Box::pin(self.reduce_rewrite(input, trigger, change, causes)).await?
            }
            TimelineFact::Branch { change } => Box::pin(self.reduce_branch(input, trigger, change, causes)).await?,
            TimelineFact::Alias { binding, receipt } => {
                Box::pin(self.reduce_alias(input, trigger, binding, receipt, causes)).await?
            }
            TimelineFact::ItemRevision { .. } => {
                return Err(MergeError::InvalidInput("canonical revisions are outputs, not producer inputs".into()));
            }
        };
        self.set_waiting(input, pending).await
    }
    pub(super) async fn unique(
        &mut self,
        key: &str,
        id: &crate::identity::ObjectId,
        evidence: &Evidence,
    ) -> Result<()> {
        if let Some(old) = self.get::<crate::identity::ObjectId>(key).await?
            && old != *id
        {
            return self.reduced(evidence, Err(Error::CreationChanged));
        }
        self.put(key, id).await?;
        Ok(())
    }
    pub async fn active_branch(&mut self) -> Result<crate::identity::ObjectId> {
        match self.get::<Versioned<BranchChange>>("branch/selected").await? {
            Some(Versioned { value: BranchChange::Select { branch, .. }, .. }) => Ok(branch),
            Some(_) => Err(MergeError::CorruptState {
                key: self.rows.key("branch/selected"),
                reason: "selection row contains membership".into(),
            }),
            None => Ok(self.initial_branch.clone()),
        }
    }
    pub async fn refresh(&mut self, item: &crate::identity::ObjectId, trigger: &Evidence) -> Result<()> {
        let Some(old) = self.item(item).await? else {
            return Ok(());
        };
        if old.alias.is_some() {
            return Ok(());
        }
        let neighbors = self.neighbors(old.placement.as_ref()).await?;
        let mut state = old.clone();
        let mut effects = Effects::default();
        self.reduced(trigger, crate::conversation::placement::position(&mut state, neighbors.as_refs(), &mut effects))?;
        self.reduced(trigger, crate::conversation::placement::publish(&mut state, &mut effects))?;
        let mut causes = state.causes();
        causes.extend(neighbors.causes());
        Box::pin(self.save_item(Some(&old), &state, trigger)).await?;
        let _ = Box::pin(self.publish_effects(effects, trigger, causes)).await?;
        // Existing pending facts retain their original evidence and are woken by
        // the same dependency rows; refresh never fabricates a new input fact.
        Ok(())
    }
}
