use super::{Address, Node, Span, Value};
use crate::claude::ingest::{
    body::{BodySink, BufferedBody},
    image_base64::Base64State,
    lexical::Event,
};
use fastctx_types::sync::cursor::{ByteCount, Count};
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Key {
    span: Span,
    small: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Place {
    name: Option<Span>,
    ordinal: Count,
    previous: Option<Address>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Frame {
    raw_start: ByteCount,
    object: bool,
    place: Place,
    source: bool,
    key: Option<Key>,
    last: Option<Address>,
    count: Count,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Scalar {
    raw_start: ByteCount,
    start: ByteCount,
    key: bool,
    number: bool,
    small: Option<Vec<u8>>,
    image: Option<(ByteCount, Base64State)>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct State {
    stack: Vec<Frame>,
    scalar: Option<Scalar>,
    pub root: Option<Address>,
}
impl State {
    pub fn validate(&self) -> io::Result<()> {
        if self.stack.len() > 128
            || self.scalar.as_ref().is_some_and(|s| s.small.as_ref().is_some_and(|v| v.len() > 64))
        {
            return Err(io::Error::other("invalid saved JSON tree state"));
        }
        Ok(())
    }
    fn place(&mut self) -> io::Result<Place> {
        if let Some(parent) = self.stack.last_mut() {
            let name = if parent.object {
                Some(parent.key.take().ok_or_else(|| io::Error::other("JSON object member has no key"))?.span)
            } else {
                None
            };
            Ok(Place { name, ordinal: parent.count, previous: parent.last })
        } else if self.root.is_none() {
            Ok(Place { name: None, ordinal: Count(0), previous: None })
        } else {
            Err(io::Error::other("JSON has multiple roots"))
        }
    }
    async fn node(
        &mut self,
        place: Place,
        value: Value,
        raw: Span,
        nodes: &mut BufferedBody,
        sink: &mut dyn BodySink,
    ) -> io::Result<()> {
        let address = Address(ByteCount(nodes.bytes()?));
        let node = Node { raw, name: place.name, ordinal: place.ordinal, previous: place.previous, value };
        let encoded = serde_json::to_vec(&node).map_err(io::Error::other)?;
        if encoded.len() > super::NODE_BYTES {
            return Err(io::Error::other("JSON descriptor exceeds its bound"));
        }
        nodes.push(&(encoded.len() as u32).to_be_bytes(), sink).await?;
        nodes.push(&encoded, sink).await?;
        if let Some(parent) = self.stack.last_mut() {
            parent.last = Some(address);
            parent.count.0 =
                parent.count.0.checked_add(1).ok_or_else(|| io::Error::other("JSON member count overflow"))?;
        } else {
            self.root = Some(address);
        }
        Ok(())
    }
    pub async fn event(
        &mut self,
        event: Event<'_>,
        through: ByteCount,
        strings: &mut BufferedBody,
        images: &mut BufferedBody,
        nodes: &mut BufferedBody,
        sink: &mut dyn BodySink,
    ) -> io::Result<()> {
        match event {
            Event::BeginObject | Event::BeginArray => {
                let source =
                    self.stack.last().and_then(|p| p.key.as_ref()).and_then(|k| k.small.as_deref()) == Some("source");
                let place = self.place()?;
                self.stack.push(Frame {
                    raw_start: back(through, 1)?,
                    object: matches!(event, Event::BeginObject),
                    place,
                    source,
                    key: None,
                    last: None,
                    count: Count(0),
                });
            }
            Event::EndObject | Event::EndArray => {
                let frame = self.stack.pop().ok_or_else(|| io::Error::other("JSON stack underflow"))?;
                let value = if frame.object {
                    Value::Object { last: frame.last, members: frame.count }
                } else {
                    Value::Array { last: frame.last, members: frame.count }
                };
                self.node(frame.place, value, extent(frame.raw_start, through)?, nodes, sink).await?;
            }
            Event::BeginString { key } => {
                let image = if !key
                    && self
                        .stack
                        .last()
                        .is_some_and(|p| p.source && p.key.as_ref().and_then(|k| k.small.as_deref()) == Some("data"))
                {
                    Some((ByteCount(images.bytes()?), Base64State::default()))
                } else {
                    None
                };
                self.scalar = Some(Scalar {
                    raw_start: back(through, 1)?,
                    start: ByteCount(strings.bytes()?),
                    key,
                    number: false,
                    small: key.then(Vec::new),
                    image,
                });
            }
            Event::BeginNumber => {
                self.scalar = Some(Scalar {
                    raw_start: through,
                    start: ByteCount(strings.bytes()?),
                    key: false,
                    number: true,
                    small: None,
                    image: None,
                })
            }
            Event::StringBytes(bytes) | Event::NumberBytes(bytes) => self.bytes(bytes, strings, images, sink).await?,
            Event::Escaped { bytes, len } => self.bytes(&bytes[..len as usize], strings, images, sink).await?,
            Event::EndString { .. } | Event::EndNumber => {
                let scalar = self.scalar.take().ok_or_else(|| io::Error::other("JSON scalar has no start"))?;
                let text = Span { offset: scalar.start, bytes: ByteCount(strings.bytes()? - scalar.start.0) };
                if scalar.key {
                    let small = scalar.small.map(String::from_utf8).transpose().map_err(io::Error::other)?;
                    self.stack.last_mut().ok_or_else(|| io::Error::other("JSON key outside object"))?.key =
                        Some(Key { span: text, small });
                } else {
                    let image = match scalar.image {
                        Some((start, state)) if state.finish().is_ok() => {
                            Some(Span { offset: start, bytes: ByteCount(images.bytes()? - start.0) })
                        }
                        _ => None,
                    };
                    let value = if scalar.number { Value::Number { text } } else { Value::String { text, image } };
                    let place = self.place()?;
                    self.node(place, value, extent(scalar.raw_start, through)?, nodes, sink).await?;
                }
            }
            Event::Bool(value) => {
                let place = self.place()?;
                self.node(
                    place,
                    Value::Bool { value },
                    extent(back(through, if value { 4 } else { 5 })?, through)?,
                    nodes,
                    sink,
                )
                .await?;
            }
            Event::Null => {
                let place = self.place()?;
                self.node(place, Value::Null, extent(back(through, 4)?, through)?, nodes, sink).await?;
            }
        }
        Ok(())
    }
    async fn bytes(
        &mut self,
        bytes: &[u8],
        strings: &mut BufferedBody,
        images: &mut BufferedBody,
        sink: &mut dyn BodySink,
    ) -> io::Result<()> {
        let scalar = self.scalar.as_mut().ok_or_else(|| io::Error::other("JSON bytes outside scalar"))?;
        strings.push(bytes, sink).await?;
        if let Some(small) = &mut scalar.small {
            if small.len() + bytes.len() <= 64 {
                small.extend_from_slice(bytes);
            } else {
                scalar.small = None;
            }
        }
        if let Some((_, state)) = &mut scalar.image {
            match state.decode(bytes) {
                Ok(decoded) => images.push(&decoded, sink).await?,
                Err(_) => scalar.image = None,
            }
        }
        Ok(())
    }
}

fn back(through: ByteCount, bytes: u64) -> io::Result<ByteCount> {
    through.0.checked_sub(bytes).map(ByteCount).ok_or_else(|| io::Error::other("JSON raw start underflow"))
}
fn extent(start: ByteCount, through: ByteCount) -> io::Result<Span> {
    Ok(Span {
        offset: start,
        bytes: ByteCount(through.0.checked_sub(start.0).ok_or_else(|| io::Error::other("JSON raw range is reversed"))?),
    })
}
