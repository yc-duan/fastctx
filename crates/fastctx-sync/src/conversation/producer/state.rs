use super::{
    api::*,
    content::Content,
    emit::Change,
    jobs::{Job, JobKind},
    keys,
    rows::Rows,
};
use crate::{
    conversation::*,
    identity::ObjectId,
    store::{ingest::SourceIngest, source_step::SourceStepTarget},
};
use fastctx_types::sync::conversation_timeline::*;
use serde::{Serialize, de::DeserializeOwned};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct Meta {
    pub version: u16,
    pub conversation: ObjectId,
    pub initial_branch: ObjectId,
}
pub(super) struct Engine<'a> {
    pub source: &'a dyn SourceIngest,
    pub target: SourceStepTarget,
    pub conversation: ObjectId,
    pub content: Content<'a>,
    pub rows: Rows,
    pub changes: Vec<Change>,
    pub inputs: Vec<MergeInput>,
    pub initial_branch: ObjectId,
}
pub(super) struct NeighborRows {
    turn: Option<TurnState>,
    parent: Option<ItemState>,
    resolved: Option<ItemState>,
    aliases: Vec<crate::catalog::RecordRef>,
}
impl NeighborRows {
    pub fn as_refs(&self) -> Neighbors<'_> {
        Neighbors { turn: self.turn.as_ref(), parent: self.parent.as_ref(), resolved_parent: self.resolved.as_ref() }
    }
    pub fn causes(&self) -> Vec<crate::catalog::RecordRef> {
        let mut result = self.aliases.clone();
        result.extend(self.turn.iter().map(|r| r.evidence.record.clone()).collect::<Vec<_>>());
        for parent in self.parent.iter().chain(self.resolved.iter()) {
            result.extend(parent.causes());
        }
        result
    }
}
impl Engine<'_> {
    pub async fn get<T: DeserializeOwned>(&mut self, key: &str) -> Result<Option<T>> {
        self.rows.get(self.source, self.target.target(), key).await
    }
    pub async fn put<T: Serialize>(&mut self, key: &str, value: &T) -> Result<bool> {
        self.rows.put(self.source, self.target.target(), key, value).await
    }
    pub async fn delete(&mut self, key: &str) -> Result<bool> {
        self.rows.delete(self.source, self.target.target(), key).await
    }
    pub async fn has(&self, prefix: &str) -> Result<bool> {
        self.rows.exists(self.source, self.target.target(), prefix).await
    }
    pub async fn first(
        &mut self,
        prefix: &str,
        after: Option<&[u8]>,
    ) -> Result<Option<crate::store::ingest_effects::SourceIndexRow>> {
        self.rows.first(self.source, self.target.target(), prefix, after).await
    }
    pub async fn item(&mut self, id: &ObjectId) -> Result<Option<ItemState>> {
        let state: Option<ItemState> = self.get(&keys::item(id)).await?;
        if state.as_ref().is_some_and(|s| s.id() != *id || s.identity.conversation() != &self.conversation) {
            return Err(MergeError::CorruptState {
                key: self.rows.key(&keys::item(id)),
                reason: "item identity differs from key or conversation".into(),
            });
        }
        Ok(state)
    }
    pub async fn effective(&mut self, id: &ObjectId) -> Result<ObjectId> {
        if let Some(state) = self.item(id).await?
            && let Some(target) = state.alias
        {
            if target == *id || self.item(&target).await?.is_some_and(|s| s.alias.is_some()) {
                return Err(MergeError::InvalidInput("invalid retained alias target".into()));
            }
            return Ok(target);
        }
        Ok(id.clone())
    }
    pub async fn neighbors(&mut self, placement: Option<&Versioned<Placement>>) -> Result<NeighborRows> {
        let mut result = NeighborRows { turn: None, parent: None, resolved: None, aliases: Vec::new() };
        if let Some(placed) = placement {
            result.turn = self.get(&format!("turn/{}", placed.value.turn.as_str())).await?;
            if let DisplaySlot::Child { parent } = &placed.value.slot {
                result.parent = self.item(parent).await?;
                if let Some(target) = result.parent.as_ref().and_then(|p| p.alias.as_ref()) {
                    result.resolved = self.item(target).await?;
                    if let Some(alias) = self.get::<MergeInput>(&format!("alias/{}", parent.as_str())).await? {
                        result.aliases.push(alias.event.evidence.record);
                        if let TimelineFact::Alias { receipt, .. } = alias.event.fact {
                            result.aliases.push(receipt);
                        }
                    }
                }
            }
        }
        Ok(result)
    }
    pub async fn save_item(&mut self, old: Option<&ItemState>, state: &ItemState, trigger: &Evidence) -> Result<()> {
        let changed = old != Some(state);
        if changed {
            self.put(&keys::item(&state.id()), state).await?;
        }
        let parent = |s: &ItemState| {
            s.placement.as_ref().and_then(|p| match &p.value.slot {
                DisplaySlot::Child { parent } => Some(parent.clone()),
                _ => None,
            })
        };
        let old_parent = old.and_then(parent);
        let new_parent = if state.alias.is_some() {
            None
        } else {
            match parent(state) {
                Some(parent) => Some(self.effective(&parent).await?),
                None => None,
            }
        };
        let mut previous = std::collections::BTreeSet::new();
        if let Some(parent) = old_parent {
            previous.insert(self.effective(&parent).await?);
            previous.insert(parent);
        }
        if let Some(parent) = old.and_then(|s| s.display_parent.clone()) {
            previous.insert(parent);
        }
        for parent in previous {
            if Some(&parent) != new_parent.as_ref() {
                self.delete(&format!("reverse/parent/{}/{}", parent.as_str(), state.id().as_str())).await?;
            }
        }
        if let Some(parent) = new_parent {
            self.put(&format!("reverse/parent/{}/{}", parent.as_str(), state.id().as_str()), &state.id()).await?;
        }
        if let Some(old_turn) = old.and_then(|s| s.placement.as_ref()).map(|p| p.value.turn.clone())
            && (state.alias.is_some() || state.placement.as_ref().map(|p| &p.value.turn) != Some(&old_turn))
        {
            self.delete(&format!("reverse/turn/{}/{}", old_turn.as_str(), state.id().as_str())).await?;
        }
        if state.alias.is_none() {
            if let Some(placed) = &state.placement {
                self.put(&format!("reverse/turn/{}/{}", placed.value.turn.as_str(), state.id().as_str()), &state.id())
                    .await?;
            }
            if let Some(run) = &state.run {
                let call = self.effective(&run.value.call).await?;
                self.put(&format!("reverse/call/{}/{}", call.as_str(), state.id().as_str()), &state.id()).await?;
                self.put(&format!("reverse/item-call/{}/{}", state.id().as_str(), call.as_str()), &call).await?;
            }
        }
        if !changed {
            return Ok(());
        }
        if old.and_then(|s| s.position.as_ref()) != state.position.as_ref()
            && self.has(&format!("reverse/parent/{}/", state.id().as_str())).await?
        {
            self.enqueue(JobKind::Children { parent: state.id() }, trigger).await?;
        }
        self.wake(&Dependency::Item(state.id()), trigger).await?;
        if old.and_then(|s| s.placement.as_ref()) != state.placement.as_ref() {
            self.wake(&Dependency::Placement(state.id()), trigger).await?;
        }
        if old.and_then(|s| s.position.as_ref()) != state.position.as_ref()
            || old.and_then(|s| s.placement.as_ref()) != state.placement.as_ref()
            || old.and_then(|s| s.alias.as_ref()) != state.alias.as_ref()
        {
            self.wake(&Dependency::Parent(state.id()), trigger).await?;
        }
        if old.map(|s| &s.bodies) != Some(&state.bodies)
            && self.has(&format!("pending/append/{}/", state.id().as_str())).await?
        {
            self.enqueue(JobKind::AppendWaiters { item: state.id() }, trigger).await?;
        }
        let prefix = format!("reverse/alias/{}/", state.id().as_str());
        if self.has(&prefix).await? {
            self.enqueue(JobKind::Aliases { target: state.id() }, trigger).await?;
        }
        Ok(())
    }
    pub async fn enqueue(&mut self, kind: JobKind, trigger: &Evidence) -> Result<()> {
        let key = format!("work/{}/{}", kind.class(), keys::hash(&kind));
        let version = self
            .get::<Job>(&key)
            .await?
            .map_or(Some(1), |j| j.version.checked_add(1))
            .ok_or(MergeError::CounterExhausted)?;
        self.put(&key, &Job { kind, trigger: trigger.clone(), phase: 0, after: None, version }).await?;
        Ok(())
    }
    pub async fn wake(&mut self, dependency: &Dependency, trigger: &Evidence) -> Result<()> {
        let dependency = keys::dependency(dependency);
        if self.has(&format!("pending/{dependency}/")).await? {
            self.enqueue(JobKind::Wake { dependency }, trigger).await?;
        }
        Ok(())
    }
    pub fn reduced<T>(
        &self,
        evidence: &Evidence,
        result: std::result::Result<T, crate::conversation::Error>,
    ) -> Result<T> {
        result.map_err(|error| MergeError::Reduction { evidence: Box::new(evidence.clone()), error })
    }
    pub async fn publish_effects(
        &mut self,
        effects: Effects,
        trigger: &Evidence,
        causes: Vec<crate::catalog::RecordRef>,
    ) -> Result<Vec<Dependency>> {
        if effects.redirect.is_some() {
            return self.reduced(trigger, Err(Error::InvalidAlias));
        }
        for prepared in effects.puts {
            let display = self.content.display(&prepared.body.primary.display).await?;
            let bytes = fastctx_types::sync::transport_codec::bounded_json(&prepared.body, 65536)
                .map_err(|_| MergeError::Limit(BudgetKind::WorkBytes))?;
            let references = prepared.body.owned_content().into_iter().cloned().collect();
            let body =
                self.content.metadata(fastctx_types::sync::metadata::MetadataCodec::BodyV1, bytes, references)?;
            let change = self.reduced(trigger, prepared.finish(body, display))?;
            self.changes.push(Change { change, trigger: trigger.clone(), causes: causes.clone() });
        }
        for change in effects.changes {
            self.changes.push(Change { change, trigger: trigger.clone(), causes: causes.clone() });
        }
        for index in effects.indexes {
            match index {
                IndexEffect::TurnReady(turn) => self.wake(&Dependency::Turn(turn), trigger).await?,
                IndexEffect::RepositionChildren(parent) => self.enqueue(JobKind::Children { parent }, trigger).await?,
                IndexEffect::RebindRelations { from, to } => {
                    self.enqueue(JobKind::Rebind { from, to }, trigger).await?
                }
                IndexEffect::Alias { .. }
                | IndexEffect::Call { .. }
                | IndexEffect::Picture(_)
                | IndexEffect::Agent(_) => {}
            }
        }
        Ok(effects.pending)
    }
}
