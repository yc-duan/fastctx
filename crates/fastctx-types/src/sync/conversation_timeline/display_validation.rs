use super::{BodySnapshot, PictureTarget, display::*};
use crate::sync::{identity::ScopeKey, manifest::ContentRef, transport_codec::bounded_json};

impl ItemDisplayV1 {
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        bounded_json(self, DISPLAY_BYTES).map_err(|_| "item display exceeds 16 KiB".into())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > DISPLAY_BYTES {
            return Err("item display exceeds 16 KiB".into());
        }
        let display: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        display.validate()?;
        Ok(display)
    }
    pub fn validate(&self) -> Result<(), String> {
        version(self.version)?;
        if (self.kind == DisplayKind::Tool) != self.tool.is_some() {
            return Err("tool display and summary differ".into());
        }
        if self.tool.as_ref().is_some_and(|tool| tool.name.is_empty()) {
            return Err("tool summary has no name".into());
        }
        if let Some(tool) = &self.tool {
            for label in
                [Some(tool.name.as_str()), tool.server.as_deref(), tool.machine.as_deref()].into_iter().flatten()
            {
                bounded_text(label, DISPLAY_LABEL_BYTES)?;
            }
            for summary in [&tool.does.text, &tool.does.command, &tool.does.path].into_iter().flatten() {
                bounded_text(summary, DISPLAY_SUMMARY_BYTES)?;
            }
        }
        let mut preview = 0usize;
        for (index, part) in self.parts.iter().enumerate() {
            part.validate()?;
            if part.index.0 != index as u64 {
                return Err("inline parts are not contiguous".into());
            }
            preview =
                preview.checked_add(part.preview.as_ref().map_or(0, String::len)).ok_or("preview size overflow")?;
            if preview > DISPLAY_PREVIEW_BYTES {
                return Err("item previews exceed 4 KiB".into());
            }
        }
        let mut total = self.parts.len() as u64;
        if let Some(more) = &self.more {
            if more.start.0 != total || more.count.0 == 0 {
                return Err("overflow parts do not continue the inline prefix".into());
            }
            total = total.checked_add(more.count.0).ok_or("part count overflow")?;
        }
        if let Some(append) = &self.append {
            if append.index.0 >= total {
                return Err("append target is outside the item".into());
            }
            if let Some(part) = usize::try_from(append.index.0).ok().and_then(|index| self.parts.get(index))
                && !part.channel.accepts_append(append.channel)
            {
                return Err("append channel differs from its part".into());
            }
        }
        bounded_json(self, DISPLAY_BYTES).map_err(|_| "item display exceeds 16 KiB")?;
        Ok(())
    }
    /// Part bodies owned by this item, plus its remaining descriptor tree. The
    /// writer walks that tree in bounded pages, retaining and authorizing all
    /// returned content and adding it to TimelineV1 references. Logical pictures
    /// belonging to another object are excluded. Full replicas follow this
    /// closure; lightweight replicas retain descriptors, not bodies or original.
    pub fn owned_content(&self) -> Vec<ContentRef> {
        let mut refs: Vec<_> = self.parts.iter().filter_map(owned_part).collect();
        refs.extend(self.more.iter().map(|more| ContentRef::Manifest { root: more.root.clone() }));
        refs
    }
    /// Checks the selected part, obtained by bounded index lookup when necessary.
    /// Updating a revision preserves every other part and reuses existing blocks.
    pub fn validate_append(&self, snapshot: &BodySnapshot, part: Option<&DisplayPart>) -> Result<(), String> {
        match (&self.append, snapshot.text_part, part) {
            (None, None, None) if snapshot.text_bytes.0 == 0 => Ok(()),
            (Some(append), Some(channel), Some(part))
                if append.channel == channel
                    && append.index == part.index
                    && part.channel.accepts_append(channel)
                    && part.bytes == snapshot.text_bytes =>
            {
                Ok(())
            }
            _ => Err("body snapshot and append target differ".into()),
        }
    }
    /// Logical cross-object picture links do not grant access to their bytes.
    /// Same-object content and descriptor subpages stay in the owner's scope.
    pub fn validate_scopes(&self, scope: &ScopeKey) -> Result<(), String> {
        for part in &self.parts {
            part.validate_scope(scope)?;
        }
        if self.more.as_ref().is_some_and(|more| &more.root.0.domain != scope) {
            return Err("display subpages cross their owner's scope".into());
        }
        Ok(())
    }
}
impl ItemDisplayPartsV1 {
    /// Additional owned bodies in this descriptor leaf; no implied access to
    /// pictures whose target is a different object.
    pub fn owned_content(&self) -> Vec<ContentRef> {
        self.parts.iter().filter_map(owned_part).collect()
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        bounded_json(self, DISPLAY_BYTES).map_err(|_| "display parts page exceeds 16 KiB".into())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > DISPLAY_BYTES {
            return Err("display parts page exceeds 16 KiB".into());
        }
        let page: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        page.validate()?;
        Ok(page)
    }
    pub fn validate(&self) -> Result<(), String> {
        version(self.version)?;
        if self.parts.is_empty() {
            return Err("empty display overflow page".into());
        }
        for (index, part) in self.parts.iter().enumerate() {
            part.validate()?;
            if self.start.0.checked_add(index as u64) != Some(part.index.0) || part.preview.is_some() {
                return Err("overflow parts must be contiguous and carry no previews".into());
            }
        }
        bounded_json(self, DISPLAY_BYTES).map_err(|_| "display parts page exceeds 16 KiB")?;
        Ok(())
    }
}
impl DisplayPart {
    pub fn validate(&self) -> Result<(), String> {
        self.content.validate()?;
        if let Some(media_type) = &self.media_type {
            bounded_text(media_type, DISPLAY_LABEL_BYTES)?;
        }
        if let ContentRef::Single { decoded_bytes, .. } = &self.content
            && u64::from(*decoded_bytes) != self.bytes.0
        {
            return Err("part length differs from its single block".into());
        }
        if self.preview.as_ref().is_some_and(|text| !self.channel.is_text() || text.len() as u64 > self.bytes.0) {
            return Err("invalid part preview".into());
        }
        if !self.channel.is_text() && self.media_type.as_ref().is_none_or(String::is_empty) {
            return Err("media part has no media type".into());
        }
        if (self.channel == DisplayChannel::Image) != self.picture.is_some() {
            return Err("image part and picture selector differ".into());
        }
        if let Some(picture) = &self.picture
            && let PictureTarget::Owned { content } = &picture.target
            && content != &self.content
        {
            return Err("owned picture differs from its content".into());
        }
        Ok(())
    }
    pub fn validate_scope(&self, owner: &ScopeKey) -> Result<(), String> {
        let expected = match self.picture.as_ref().map(|picture| &picture.target) {
            Some(PictureTarget::Object { object }) => &object.scope,
            _ => owner,
        };
        if &self.content.id().domain != expected {
            return Err("part reference crosses its authorized target scope".into());
        }
        Ok(())
    }
}
fn owned_part(part: &DisplayPart) -> Option<ContentRef> {
    (!matches!(part.picture.as_ref().map(|picture| &picture.target), Some(PictureTarget::Object { .. })))
        .then(|| part.content.clone())
}
fn bounded_text(text: &str, limit: usize) -> Result<(), String> {
    if text.len() > limit {
        return Err(format!("display summary field exceeds {limit} bytes"));
    }
    Ok(())
}
fn version(version: u16) -> Result<(), String> {
    if version != DISPLAY_VERSION {
        return Err("unsupported item display version".into());
    }
    Ok(())
}
