use super::{api::*, state::Engine};
use super::{
    jobs::{JobKind, Relation},
    keys,
};
use crate::conversation::*;
use fastctx_types::sync::conversation_timeline::*;

impl Engine<'_> {
    pub(super) async fn reduce_turn(
        &mut self,
        input: &MergeInput,
        trigger: &Evidence,
        declaration: &TurnDeclaration,
        causes: Vec<crate::catalog::RecordRef>,
    ) -> Result<Vec<Dependency>> {
        let evidence = &input.event.evidence;
        Ok({
            if declaration.key.conversation != self.conversation {
                return self.reduced(evidence, Err(Error::WrongConversation));
            }
            let key = format!("turn/{}", declaration.key.id().as_str());
            self.unique(&format!("turn-order/{:020}", declaration.order.0), &declaration.key.id(), evidence).await?;
            let old: Option<TurnState> = self.get(&key).await?;
            let (state, effects) = self.reduced(evidence, turn(old.as_ref(), declaration, evidence))?;
            if self.put(&key, &state).await? {
                self.wake(&Dependency::Turn(declaration.key.id()), trigger).await?;
            }
            Box::pin(self.publish_effects(effects, trigger, causes)).await?
        })
    }
    pub(super) async fn reduce_image(
        &mut self,
        input: &MergeInput,
        trigger: &Evidence,
        link: &PictureLink,
        causes: Vec<crate::catalog::RecordRef>,
    ) -> Result<Vec<Dependency>> {
        let evidence = &input.event.evidence;
        Ok({
            let Some(item) = self.item(&link.item).await? else {
                return Ok(vec![Dependency::Item(link.item.clone())]);
            };
            let selector = keys::hash(&link.selector);
            let key = format!("picture/{}/{}", link.item.as_str(), selector);
            let old: Option<Versioned<PictureLink>> = self.get(&key).await?;
            let reduction = self.reduced(evidence, picture(&item, old.as_ref(), link, evidence))?;
            if let Some(state) = reduction.state {
                self.put(&key, &state).await?;
                if state.evidence == *evidence {
                    self.put(
                        &format!("reverse/picture/{}/{}", link.item.as_str(), selector),
                        &Relation { key: key.clone(), input: input.clone() },
                    )
                    .await?;
                }
            }
            Box::pin(self.publish_effects(reduction.effects, trigger, causes)).await?
        })
    }
    pub(super) async fn reduce_agent(
        &mut self,
        input: &MergeInput,
        trigger: &Evidence,
        link: &AgentLink,
        causes: Vec<crate::catalog::RecordRef>,
    ) -> Result<Vec<Dependency>> {
        let evidence = &input.event.evidence;
        Ok({
            let key = format!("agent/{}/{}", keys::hash(&link.parent), keys::hash(&link.child));
            let old: Option<AgentState> = self.get(&key).await?;
            let parent_key = format!("agent-parent/{}", keys::hash(&link.child));
            if let Some(parent) = self.get::<crate::identity::ObjectKey>(&parent_key).await?
                && parent != link.parent
            {
                return self.reduced(evidence, Err(Error::WrongKey));
            }
            let mut parent = link.parent.clone();
            let mut visited = std::collections::BTreeSet::new();
            while let Some(known) =
                self.get::<crate::identity::ObjectKey>(&format!("agent-parent/{}", keys::hash(&parent))).await?
            {
                if !visited.insert(parent.clone()) || known == link.child {
                    return self.reduced(evidence, Err(Error::Cycle));
                }
                if !input.agent_ancestors.contains(&known.object) {
                    return Err(MergeError::InvalidInput(
                        "verified agent ancestor chain omits a retained parent".into(),
                    ));
                }
                parent = known;
            }
            let (state, effects) =
                self.reduced(evidence, agent(old.as_ref(), link, evidence, &input.agent_ancestors))?;
            self.put(&key, &state).await?;
            self.put(&parent_key, &link.parent).await?;
            Box::pin(self.publish_effects(effects, trigger, causes)).await?
        })
    }
    pub(super) async fn reduce_rewrite(
        &mut self,
        input: &MergeInput,
        trigger: &Evidence,
        change: &HostRewritten,
        causes: Vec<crate::catalog::RecordRef>,
    ) -> Result<Vec<Dependency>> {
        let evidence = &input.event.evidence;
        Ok({
            let key = format!("source/{}", change.source.as_str());
            let old: SourceState = self
                .get(&key)
                .await?
                .ok_or_else(|| MergeError::InvalidInput("rewrite source has no initial generation".into()))?;
            let reduction = self.reduced(evidence, rewrite(&old, change, evidence))?;
            if self.put(&key, &reduction.state).await? {
                self.wake(
                    &Dependency::Generation {
                        source: change.source.clone(),
                        previous: reduction.state.current.clone(),
                    },
                    trigger,
                )
                .await?;
            }
            Box::pin(self.publish_effects(reduction.effects, trigger, causes)).await?
        })
    }
    pub(super) async fn reduce_branch(
        &mut self,
        input: &MergeInput,
        trigger: &Evidence,
        change: &BranchChange,
        mut causes: Vec<crate::catalog::RecordRef>,
    ) -> Result<Vec<Dependency>> {
        let evidence = &input.event.evidence;
        Ok({
            let active = self.active_branch().await?;
            let (key, item) = match change {
                BranchChange::Select { .. } => ("branch/selected".to_owned(), None),
                BranchChange::Membership { item, branch, rollback, .. } => {
                    causes.extend(rollback.iter().cloned());
                    (format!("branch/member/{}/{}", branch.as_str(), item.as_str()), self.item(item).await?)
                }
            };
            let old: Option<Versioned<BranchChange>> = self.get(&key).await?;
            let reduction = self.reduced(evidence, branch(old.as_ref(), item.as_ref(), &active, change, evidence))?;
            if let Some(state) = &reduction.state {
                let changed = self.put(&key, state).await?;
                match &state.value {
                    BranchChange::Membership { item, branch, .. } if state.evidence == *evidence => {
                        self.put(
                            &format!("reverse/member/{}/{}", item.as_str(), branch.as_str()),
                            &Relation { key: key.clone(), input: input.clone() },
                        )
                        .await?;
                    }
                    BranchChange::Select { branch, .. } if changed => {
                        self.enqueue(JobKind::Members { branch: branch.clone() }, trigger).await?;
                    }
                    _ => {}
                }
            }
            if let Some(state) = &reduction.item {
                causes.extend(state.causes());
                Box::pin(self.save_item(item.as_ref(), state, trigger)).await?;
            }
            Box::pin(self.publish_effects(reduction.effects, trigger, causes)).await?
        })
    }
}
