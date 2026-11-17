use super::facts::*;
use crate::sync::{PAGE_BYTES, identity::ScopeKey, manifest::ContentRef, transport_codec::bounded_json};

impl DisplayFactsV1 {
    pub fn owned_content(&self) -> Vec<ContentRef> {
        [&self.turn, &self.native, &self.semantic, &self.event].into_iter().flatten().cloned().collect()
    }
    pub fn validate(&self) -> Result<(), String> {
        for reference in self.owned_content() {
            descriptor(&reference)?;
        }
        Ok(())
    }
    pub fn validate_scope(&self, scope: &ScopeKey) -> Result<(), String> {
        scoped(scope, &self.owned_content())
    }
}
impl ItemFactBodyV1 {
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        bounded_json(self, PAGE_BYTES as usize).map_err(|_| "item facts exceed one metadata page".into())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > PAGE_BYTES as usize {
            return Err("item facts exceed one metadata page".into());
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.version != FACTS_VERSION {
            return Err("unsupported item facts version".into());
        }
        if let ItemFactV1::Parts { part_kind, start, parts } = &self.fact
            && (parts.len() > FACT_PARTS
                || parts.is_empty() && start.0 != 0
                || start.0.checked_add(parts.len() as u64).is_none()
                || parts.iter().any(|part| part.kind() != *part_kind))
        {
            return Err("facts leaf has an invalid kind, range or page bound".into());
        }
        match &self.fact {
            ItemFactV1::Native { context: Some(reference), .. }
            | ItemFactV1::NativeRow { supersedes: Some(reference), .. } => descriptor(reference)?,
            ItemFactV1::Tool { detail, output, does, .. } => {
                descriptor(detail)?;
                if let Some(reference) = output {
                    descriptor(reference)?;
                }
                match does.as_deref() {
                    Some(ToolActionV1::Edit { changes }) => descriptor(changes)?,
                    Some(ToolActionV1::Agent { agents: Some(agents), .. }) => descriptor(agents)?,
                    _ => {}
                }
            }
            ItemFactV1::Session { event } => match event {
                SessionFactV1::MessageUsage { items, .. } => descriptor(items)?,
                SessionFactV1::ContextRow { row } => descriptor(row)?,
                SessionFactV1::Compaction {
                    preserved: Some(PreservedContextV1 { selection: PreservedSelectionV1::Messages { uuids }, .. }),
                    ..
                } => descriptor(uuids)?,
                _ => {}
            },
            ItemFactV1::Parts { parts, .. } => {
                for part in parts {
                    if let FactPartV1::Change { diff: FactTextV1::Parts { parts }, .. } = part {
                        descriptor(parts)?;
                    }
                }
            }
            _ => {}
        }
        for reference in self.owned_content() {
            reference.validate()?;
        }
        bounded_json(self, PAGE_BYTES as usize).map_err(|_| "item facts exceed one metadata page")?;
        Ok(())
    }
    /// Readers verify each fetched child against its parent's advertised range
    /// and the requested part kind before interpreting any leaf content.
    pub fn validate_parts(&self, expected: FactPartKindV1, start: u64, count: u64) -> Result<(), String> {
        let (kind, own_start, own_count) = match &self.fact {
            ItemFactV1::Parts { part_kind, start, parts } => (*part_kind, start.0, parts.len() as u64),
            _ => return Err("facts reference does not point to a parts descriptor".into()),
        };
        if kind != expected || own_start != start || own_count != count {
            return Err("facts part kind or range differs".into());
        }
        Ok(())
    }
    pub fn validate_scope(&self, scope: &ScopeKey) -> Result<(), String> {
        scoped(scope, &self.owned_content())
    }
    pub fn owned_content(&self) -> Vec<ContentRef> {
        let mut refs = Vec::new();
        match &self.fact {
            ItemFactV1::TurnMapping { .. } => {}
            ItemFactV1::Native { context, .. } => refs.extend(context.iter().cloned()),
            ItemFactV1::NativeRow { host_kind, origin, supersedes, .. } => {
                refs.push(host_kind.clone());
                refs.extend(origin.iter().chain(supersedes).cloned());
            }
            ItemFactV1::Tool { name, server, does, input, output, structured, detail, .. } => {
                refs.push(detail.clone());
                refs.extend(name.iter().chain(input).chain(server).chain(output).chain(structured).cloned());
                if let Some(does) = does {
                    does.references(&mut refs);
                }
            }
            ItemFactV1::ToolDetail { requested, stored, .. } => {
                refs.extend(requested.iter().cloned());
                if let Some(stored) = stored {
                    refs.push(stored.block.clone());
                    refs.extend(stored.structured.iter().chain(&stored.colleague).chain(&stored.agent).cloned());
                }
            }
            ItemFactV1::Message { model, stop_reason, usage, detail, .. } => {
                refs.push(detail.clone());
                refs.extend(model.iter().chain(stop_reason).chain(usage).cloned());
            }
            ItemFactV1::Session { event } => event.references(&mut refs),
            ItemFactV1::Parts { parts, .. } => {
                for part in parts {
                    part.references(&mut refs);
                }
            }
        }
        refs
    }
}
impl ToolActionV1 {
    fn references(&self, refs: &mut Vec<ContentRef>) {
        use ToolActionV1::*;
        match self {
            Command { command, cwd, job } => {
                refs.push(command.clone());
                refs.extend(cwd.iter().chain(job).cloned());
            }
            Edit { changes } => refs.push(changes.clone()),
            Search { query } => refs.push(query.clone()),
            Image { path } => refs.extend(path.iter().cloned()),
            Colleague { to, task } => refs.extend(to.iter().chain(task).cloned()),
            Agent { agents, agent_kind, task, .. } => refs.extend(agents.iter().chain(agent_kind).chain(task).cloned()),
            Call => {}
        }
    }
}
impl FactPartV1 {
    pub fn kind(&self) -> FactPartKindV1 {
        match self {
            Self::Output { .. } => FactPartKindV1::Output,
            Self::Change { .. } => FactPartKindV1::Change,
            Self::NativeItem { .. } => FactPartKindV1::NativeItem,
            Self::Text { .. } => FactPartKindV1::Text,
            Self::NativeId { .. } => FactPartKindV1::NativeId,
        }
    }
    fn references(&self, refs: &mut Vec<ContentRef>) {
        match self {
            Self::Output { output } => match output {
                ToolOutputV1::Text { text } => refs.push(text.clone()),
                ToolOutputV1::Image { mime, base64 } => refs.extend([mime, base64].into_iter().cloned()),
                ToolOutputV1::ImageUrl { url } | ToolOutputV1::AudioUrl { url } => refs.push(url.clone()),
                ToolOutputV1::Other { block } => refs.push(block.clone()),
            },
            Self::Change { path, diff, how } => {
                refs.push(path.clone());
                refs.push(match diff {
                    FactTextV1::Content { content } => content.clone(),
                    FactTextV1::Parts { parts } => parts.clone(),
                });
                if let FactChangeV1::Moved { to } = how {
                    refs.push(to.clone());
                }
            }
            Self::Text { text } => refs.push(text.clone()),
            Self::NativeItem { .. } | Self::NativeId { .. } => {}
        }
    }
}
impl SessionFactV1 {
    fn references(&self, refs: &mut Vec<ContentRef>) {
        match self {
            Self::Title { title, .. } => refs.extend(title.iter().cloned()),
            Self::MessageUsage { model, stop_reason, usage, items, .. } => {
                refs.extend([model, items].into_iter().cloned());
                refs.extend(stop_reason.iter().chain(usage).cloned());
            }
            Self::Compaction { preserved, detail, .. } => {
                refs.push(detail.clone());
                if let Some(PreservedContextV1 { selection: PreservedSelectionV1::Messages { uuids }, .. }) = preserved
                {
                    refs.push(uuids.clone());
                }
            }
            Self::ContextRow { row } => refs.push(row.clone()),
            Self::ExchangeEnded { reason, .. } => refs.push(reason.clone()),
            Self::Hidden { host_kind, detail } | Self::Other { host_kind, detail } => {
                refs.extend([host_kind, detail].into_iter().cloned())
            }
            Self::Diagnostic { message, detail } => {
                refs.push(message.clone());
                refs.extend(detail.iter().cloned());
            }
        }
    }
}
fn descriptor(reference: &ContentRef) -> Result<(), String> {
    reference.validate()?;
    if !matches!(reference, ContentRef::Manifest { .. }) {
        return Err("public facts require a typed metadata descriptor".into());
    }
    Ok(())
}
fn scoped(scope: &ScopeKey, refs: &[ContentRef]) -> Result<(), String> {
    for reference in refs {
        reference.validate()?;
        if &reference.id().domain != scope {
            return Err("public facts cross their item's content scope".into());
        }
    }
    Ok(())
}
