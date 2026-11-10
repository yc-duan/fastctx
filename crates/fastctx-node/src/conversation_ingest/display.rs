//! Bounded display construction. Overflow pages keep every part in order; only
//! previews and folded summaries are shortened, never the referenced bodies.
use super::{
    display_pages::{DescriptorPage, join_parts},
    step_content::StepContent,
};
use fastctx_sync::{cursor::Count, manifest::ContentRef, store::Error};
use fastctx_types::sync::conversation_timeline::{
    DisplayKind, DisplayPart, DisplayPartsRef, DisplayStatus, ItemDisplayPartsV1, ItemDisplayV1, ToolSummary,
    display_preview,
};
use serde::{Deserialize, Serialize};

const FANOUT: usize = 32;
const LEAF_PARTS: usize = 8;
const OVERFLOW_ROOM: usize = 512;

#[derive(Clone, Serialize, Deserialize)]
pub struct Builder {
    display: ItemDisplayV1,
    next: Count,
    previews: usize,
    overflow: bool,
    pending: Vec<DisplayPart>,
    levels: Vec<Vec<DisplayPartsRef>>,
}
impl Builder {
    pub fn new(kind: DisplayKind, tool: Option<ToolSummary>, status: DisplayStatus) -> Result<Self, Error> {
        let mut display = ItemDisplayV1 {
            version: 1,
            kind,
            tool,
            status: Some(status),
            facts: None,
            append: None,
            parts: Vec::new(),
            more: None,
        };
        if let Some(tool) = &mut display.tool {
            clip_tool(tool, 1024);
        }
        for _ in 0..12 {
            if display.encode().is_ok() {
                break;
            }
            let Some(tool) = &mut display.tool else { break };
            shorten_tool(tool);
        }
        display.encode().map_err(Error::Invalid)?;
        Ok(Self { display, next: Count(0), previews: 0, overflow: false, pending: Vec::new(), levels: Vec::new() })
    }
    pub fn next_index(&self) -> Count {
        self.next
    }
    pub fn preview_bytes(&self) -> usize {
        4096_usize.saturating_sub(self.previews)
    }
    pub async fn push(&mut self, content: &StepContent<'_>, mut part: DisplayPart) -> Result<(), Error> {
        if part.index != self.next {
            return Err(Error::Invalid("display parts are not contiguous".into()));
        }
        if let Some(preview) = &mut part.preview {
            *preview = display_preview(preview, self.preview_bytes());
        }
        part.validate().map_err(Error::Invalid)?;
        self.next.0 =
            self.next.0.checked_add(1).ok_or_else(|| Error::Invalid("display part index exhausted".into()))?;
        if !self.overflow && self.display.parts.len() < 4 {
            loop {
                let mut inline = self.display.clone();
                inline.parts.push(part.clone());
                if inline.encode().is_ok_and(|bytes| bytes.len() <= 16 * 1024 - OVERFLOW_ROOM) {
                    self.previews += part.preview.as_ref().map_or(0, String::len);
                    self.display = inline;
                    return Ok(());
                }
                // JSON escaping can exceed the encoded bound before the UTF-8
                // preview budget. Keep a shorter valid preview in the first page.
                let Some(preview) = &mut part.preview else { break };
                if preview.is_empty() {
                    break;
                }
                *preview = display_preview(preview, preview.len() / 2);
            }
        }
        self.overflow = true;
        // Overflow leaves have no previews. The folded page owns the one global
        // preview budget; expansion reads the referenced content unchanged.
        part.preview = None;
        ItemDisplayPartsV1 { version: 1, start: part.index, parts: vec![part.clone()] }
            .encode()
            .map_err(Error::Invalid)?;
        self.pending.push(part);
        if self.leaf().encode().is_err() {
            let last = self.pending.pop().expect("the new part is present");
            self.flush_leaf(content).await?;
            self.pending.push(last);
        }
        if self.pending.len() == LEAF_PARTS {
            self.flush_leaf(content).await?;
        }
        Ok(())
    }
    fn leaf(&self) -> ItemDisplayPartsV1 {
        ItemDisplayPartsV1 {
            version: 1,
            start: self.pending.first().map_or(self.next, |part| part.index),
            parts: self.pending.clone(),
        }
    }
    async fn flush_leaf(&mut self, content: &StepContent<'_>) -> Result<(), Error> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let plan = DescriptorPage::parts(content.target().target().custody.object.scope.clone(), &self.leaf())
            .map_err(invalid)?;
        let payload = content.put_block(plan.payload().to_vec()).await?;
        let (page, reference) = plan.parts_manifest(payload).map_err(invalid)?;
        content.put_manifest(page).await?;
        self.pending.clear();
        self.push_reference(content, 0, reference).await
    }
    async fn push_reference(
        &mut self,
        content: &StepContent<'_>,
        mut level: usize,
        mut reference: DisplayPartsRef,
    ) -> Result<(), Error> {
        loop {
            if level >= 64 {
                return Err(Error::Invalid("display overflow tree exceeds its depth bound".into()));
            }
            if self.levels.len() == level {
                self.levels.push(Vec::new());
            }
            self.levels[level].push(reference);
            if self.levels[level].len() < FANOUT {
                return Ok(());
            }
            let group = std::mem::take(&mut self.levels[level]);
            reference = join(content, &group).await?;
            level += 1;
        }
    }
    pub async fn finish(mut self, content: &StepContent<'_>) -> Result<(ContentRef, ItemDisplayV1), Error> {
        self.flush_leaf(content).await?;
        let mut level = 0;
        while level < self.levels.len() {
            let group = std::mem::take(&mut self.levels[level]);
            if !group.is_empty() {
                let reference = join(content, &group).await?;
                if self.levels.iter().skip(level + 1).all(Vec::is_empty) {
                    self.display.more = Some(reference);
                    break;
                }
                self.push_reference(content, level + 1, reference).await?;
            }
            level += 1;
        }
        let plan = DescriptorPage::display(content.target().target().custody.object.scope.clone(), &self.display)
            .map_err(invalid)?;
        let payload = content.put_block(plan.payload().to_vec()).await?;
        let page = plan.manifest(payload).map_err(invalid)?;
        let root = ContentRef::Manifest { root: page.root.clone() };
        content.put_manifest(page).await?;
        Ok((root, self.display))
    }
}
async fn join(content: &StepContent<'_>, group: &[DisplayPartsRef]) -> Result<DisplayPartsRef, Error> {
    let joined = join_parts(&content.target().target().custody.object.scope, group).map_err(invalid)?;
    if let Some(page) = joined.page {
        content.put_manifest(page).await?;
    }
    Ok(joined.reference)
}
fn invalid(error: std::io::Error) -> Error {
    Error::Invalid(error.to_string())
}
fn clip(value: &mut Option<String>, max: usize) {
    if let Some(text) = value {
        *text = display_preview(text, max);
    }
}
fn clip_tool(tool: &mut ToolSummary, max: usize) {
    tool.name = display_preview(&tool.name, max.min(256));
    clip(&mut tool.server, max.min(256));
    clip(&mut tool.machine, max.min(256));
    clip(&mut tool.does.text, max);
    clip(&mut tool.does.command, max);
    clip(&mut tool.does.path, max);
}
fn shorten_tool(tool: &mut ToolSummary) {
    tool.name = display_preview(&tool.name, tool.name.len() / 2);
    for text in [&mut tool.server, &mut tool.machine, &mut tool.does.text, &mut tool.does.command, &mut tool.does.path]
        .into_iter()
        .flatten()
    {
        *text = display_preview(text, text.len() / 2);
    }
}
