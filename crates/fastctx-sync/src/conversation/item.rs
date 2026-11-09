use super::*;
use fastctx_types::sync::{conversation_timeline::*, cursor::ByteCount, manifest::ContentRef};

pub fn observe(
    old: Option<&ItemState>,
    observation: &Observation,
    evidence: &Evidence,
    neighbors: Neighbors<'_>,
) -> Result<ItemReduction, Error> {
    if observation.authority == BodyAuthority::HostFinal && !observation.body.complete {
        return Err(Error::FinalText);
    }
    observation.body.original.validate().map_err(Error::InvalidContent)?;
    observation.body.display.validate().map_err(Error::InvalidContent)?;
    if observation.created.0 == 0 {
        return Err(Error::InvalidPlacement);
    }
    let mut state = old.cloned().unwrap_or_else(|| ItemState::new(observation.identity.clone(), observation.created));
    if state.identity != observation.identity {
        return Err(Error::WrongKey);
    }
    if state.created != observation.created {
        return Err(Error::CreationChanged);
    }
    let mut effects = Effects::default();
    if let Some(target) = &state.alias {
        effects.redirect = Some(target.clone());
        return Ok(ItemReduction { state, effects });
    }
    let incoming = Versioned { evidence: evidence.clone(), value: observation.body.clone() };
    if state.bodies.get(&observation.authority).map(|old| old.accepts(&incoming)).transpose()?.unwrap_or(true) {
        state.bodies.insert(observation.authority, incoming);
    }
    if let Some(placed) = &observation.placement {
        placement::choose(&mut state.placement, Versioned { evidence: evidence.clone(), value: placed.clone() })?;
    }
    placement::position(&mut state, neighbors, &mut effects)?;
    placement::publish(&mut state, &mut effects)?;
    Ok(ItemReduction { state, effects })
}

pub fn place(
    old: &ItemState,
    placed: &Placement,
    evidence: &Evidence,
    neighbors: Neighbors<'_>,
) -> Result<ItemReduction, Error> {
    let mut state = old.clone();
    let mut effects = Effects::default();
    placement::choose(&mut state.placement, Versioned { evidence: evidence.clone(), value: placed.clone() })?;
    placement::position(&mut state, neighbors, &mut effects)?;
    placement::publish(&mut state, &mut effects)?;
    Ok(ItemReduction { state, effects })
}

pub fn run(old: &ItemState, link: &RunLink, evidence: &Evidence) -> Result<ItemReduction, Error> {
    use fastctx_types::sync::conversation_identity::ItemRole;
    if !matches!(old.identity.role(), ItemRole::ToolCall | ItemRole::ToolResult)
        || old.run.as_ref().is_some_and(|run| run.value.call != link.call)
    {
        return Err(Error::WrongKey);
    }
    if let Some(summary) = &link.summary {
        summary.validate().map_err(Error::InvalidContent)?;
    }
    let mut state = old.clone();
    let mut effects = Effects::default();
    let incoming = Versioned { evidence: evidence.clone(), value: link.clone() };
    if state.run.as_ref().map(|old| old.accepts(&incoming)).transpose()?.unwrap_or(true) {
        state.run = Some(incoming);
        effects.indexes.push(IndexEffect::Call { call: link.call.clone(), item: state.id() });
    }
    placement::publish(&mut state, &mut effects)?;
    Ok(ItemReduction { state, effects })
}

#[derive(Clone, Debug)]
pub struct AppendPlan {
    pub item: fastctx_types::sync::identity::ObjectId,
    pub previous: ContentRef,
    pub part: fastctx_types::session::Part,
    pub chunk: ContentRef,
    pub offset: ByteCount,
    pub bytes: ByteCount,
    authority: BodyAuthority,
    evidence: Evidence,
    base: fastctx_types::sync::identity::OperationId,
    input: TextAppend,
}
#[derive(Clone, Debug)]
pub enum AppendOutcome {
    Ready(Box<AppendPlan>),
    Pending(Dependency),
    Superseded,
    Redirect(fastctx_types::sync::identity::ObjectId),
    Replayed,
}

pub fn append(old: &ItemState, input: &TextAppend, evidence: &Evidence) -> Result<AppendOutcome, Error> {
    if old.id() != input.item {
        return Err(Error::WrongKey);
    }
    if let Some(target) = &old.alias {
        return Ok(AppendOutcome::Redirect(target.clone()));
    }
    if !matches!(input.authority, BodyAuthority::HostDraft) {
        return Err(Error::FinalText);
    }
    let Some(body) = old.bodies.get(&input.authority) else {
        return Ok(AppendOutcome::Pending(Dependency::Item(input.item.clone())));
    };
    if old.bodies.contains_key(&BodyAuthority::HostFinal) || body.value.complete {
        return Ok(AppendOutcome::Superseded);
    }
    if let Some(last) = &old.last_append
        && last.evidence.operation == evidence.operation
    {
        if last.evidence != *evidence || last.value != *input {
            return Err(Error::ConflictingReplay);
        }
        return Ok(AppendOutcome::Replayed);
    }
    if body.evidence.operation == evidence.operation {
        return Err(Error::ConflictingReplay);
    }
    if evidence.accepted <= body.evidence.accepted {
        return Ok(AppendOutcome::Superseded);
    }
    if body.evidence.operation != input.previous {
        return Ok(AppendOutcome::Pending(Dependency::Append {
            item: input.item.clone(),
            previous: input.previous.clone(),
        }));
    }
    if body.value.text_part != Some(input.part) {
        return Err(Error::AppendPart);
    }
    if input.offset != body.value.text_bytes {
        return Err(Error::AppendLength);
    }
    input.offset.0.checked_add(input.bytes.0).ok_or(Error::CounterExhausted)?;
    input.chunk.validate().map_err(Error::InvalidContent)?;
    if let ContentRef::Single { decoded_bytes, .. } = input.chunk
        && u64::from(decoded_bytes) != input.bytes.0
    {
        return Err(Error::AppendLength);
    }
    Ok(AppendOutcome::Ready(Box::new(AppendPlan {
        item: input.item.clone(),
        previous: body.value.display.clone(),
        part: input.part,
        chunk: input.chunk.clone(),
        offset: input.offset,
        bytes: input.bytes,
        authority: input.authority,
        evidence: evidence.clone(),
        base: input.previous.clone(),
        input: input.clone(),
    })))
}
impl AppendPlan {
    /// The caller appends to `part` in the projected body's persistent text tree,
    /// retaining its other fields and old blocks. It never concatenates a chunk
    /// onto serialized JSON. A stale plan cannot overwrite a newer item.
    pub fn finish(self, old: &ItemState, result: ContentRef) -> Result<ItemReduction, Error> {
        result.validate().map_err(Error::InvalidContent)?;
        let current = old.bodies.get(&self.authority).ok_or(Error::AppendBase)?;
        if old.id() != self.item
            || current.evidence.operation != self.base
            || current.value.display != self.previous
            || old.bodies.contains_key(&BodyAuthority::HostFinal)
            || old.alias.is_some()
        {
            return Err(Error::AppendBase);
        }
        let mut state = old.clone();
        let mut effects = Effects::default();
        let mut body = current.value.clone();
        body.display = result;
        body.text_bytes = ByteCount(self.offset.0.checked_add(self.bytes.0).ok_or(Error::CounterExhausted)?);
        state.last_append = Some(Versioned { evidence: self.evidence.clone(), value: self.input });
        state.bodies.insert(self.authority, Versioned { evidence: self.evidence, value: body });
        placement::publish(&mut state, &mut effects)?;
        Ok(ItemReduction { state, effects })
    }
}
