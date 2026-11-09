//! Decode media only after the complete row establishes its final type. This
//! handles data-before-type and duplicate fields without speculative image writes.
use super::{
    document::{
        Address, Span, Value,
        read::{Reader, exact},
    },
    image_base64::Base64State,
    lexical::WINDOW_BYTES,
};
use fastctx_types::sync::cursor::ByteCount;
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageCursor {
    source: Span,
    encoded: ByteCount,
    base64: Base64State,
}
pub struct ImageSource {
    data: Span,
    media_type: String,
}
impl ImageSource {
    /// A document block always belongs to file semantics, even if its media type
    /// describes an image. Unknown block types require an explicit image MIME.
    pub async fn from_block(reader: &Reader<'_>, block: Address) -> io::Result<Option<Self>> {
        let kind = text(reader, block, "type").await?;
        if kind.as_deref() == Some("document") {
            return Ok(None);
        }
        let Some((source, node)) = reader.field(block, "source").await? else {
            return Ok(None);
        };
        if !matches!(node.value, Value::Object { .. })
            || text(reader, source, "type").await?.as_deref() != Some("base64")
        {
            return Ok(None);
        }
        let Some(media_type) = text(reader, source, "media_type").await? else {
            return Ok(None);
        };
        if kind.as_deref() != Some("image") && !media_type.starts_with("image/") {
            return Ok(None);
        }
        let Some((_, node)) = reader.field(source, "data").await? else {
            return Ok(None);
        };
        let Value::String { text: data } = node.value else {
            return Ok(None);
        };
        Ok(Some(Self { data, media_type }))
    }
    pub fn encoded_span(&self) -> Span {
        self.data
    }
    pub fn media_type(&self) -> &str {
        &self.media_type
    }
    pub fn start(&self) -> ImageCursor {
        ImageCursor { source: self.data, encoded: ByteCount(0), base64: Base64State::default() }
    }
    /// The returned bytes are at most 48 KiB. Invalid data is an error, never a
    /// partial valid image; the caller retains the original and abandons this body.
    pub async fn read_window(
        &self,
        reader: &Reader<'_>,
        cursor: &mut ImageCursor,
        limit: usize,
    ) -> io::Result<Vec<u8>> {
        if limit == 0 || limit > WINDOW_BYTES || cursor.source != self.data || cursor.encoded > self.data.bytes {
            return Err(io::Error::other("invalid image cursor or window"));
        }
        let take = (self.data.bytes.0 - cursor.encoded.0).min(limit as u64) as usize;
        let bytes =
            exact(reader.input, &reader.document.bodies.strings, self.data.offset.0 + cursor.encoded.0, take).await?;
        let decoded = cursor.base64.decode(&bytes)?;
        cursor.encoded.0 += take as u64;
        if cursor.encoded == self.data.bytes {
            cursor.base64.finish()?;
        }
        Ok(decoded)
    }
    pub fn complete(&self, cursor: &ImageCursor) -> bool {
        cursor.source == self.data && cursor.encoded == self.data.bytes && cursor.base64.finish().is_ok()
    }
}
async fn text(reader: &Reader<'_>, object: Address, name: &str) -> io::Result<Option<String>> {
    match reader.field(object, name).await? {
        Some((_, node)) => match node.value {
            Value::String { text } => reader.text(text, 256).await,
            _ => Ok(None),
        },
        None => Ok(None),
    }
}
