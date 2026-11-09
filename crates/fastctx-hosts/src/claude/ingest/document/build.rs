use super::{Address, Node, Shape, Span, Unindexed, Value, select::Selection};
use crate::claude::ingest::{
    body::{BodySink, BufferedBody},
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
    capture: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Frame {
    raw_start: ByteCount,
    object: bool,
    place: Place,
    selection: Selection,
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
    store: bool,
    small: Option<Vec<u8>>,
    place: Option<Place>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct State {
    stack: Vec<Frame>,
    scalar: Option<Scalar>,
    pub root: Option<Address>,
    exhausted: bool,
}
impl State {
    pub fn validate(&self) -> io::Result<()> {
        if self.stack.len() > 128
            || self.scalar.as_ref().is_some_and(|s| s.small.as_ref().is_some_and(|v| v.len() > 64))
            || self.stack.iter().any(|f| f.key.as_ref().is_some_and(|k| k.small.as_ref().is_some_and(|v| v.len() > 64)))
        {
            return Err(io::Error::other("invalid saved JSON tree state"));
        }
        Ok(())
    }
    fn place(&mut self) -> io::Result<(Place, Selection)> {
        if let Some(parent) = self.stack.last_mut() {
            let key = if parent.object {
                Some(parent.key.take().ok_or_else(|| io::Error::other("JSON object member has no key"))?)
            } else {
                None
            };
            let selection = parent.selection.child(key.as_ref().and_then(|k| k.small.as_deref()));
            Ok((
                Place {
                    name: key.map(|k| k.span),
                    ordinal: parent.count,
                    previous: parent.last,
                    capture: parent.selection != Selection::Opaque && !self.exhausted,
                },
                selection,
            ))
        } else if self.root.is_none() {
            Ok((Place { name: None, ordinal: Count(0), previous: None, capture: true }, Selection::Row))
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
        let root = self.stack.is_empty();
        let mut address = None;
        if root || place.capture && !self.exhausted {
            let value = if root && self.exhausted {
                Value::Opaque { shape: value.shape(), reason: Unindexed::RowBudget }
            } else {
                value
            };
            let mut node = Node { raw, name: place.name, ordinal: place.ordinal, previous: place.previous, value };
            let mut encoded = super::codec::encode(&node);
            let limit = super::index_limit(raw.end()?).saturating_sub(super::NODE_BYTES as u64 + 4);
            if nodes.bytes()?.saturating_add(encoded.len() as u64 + 4) > limit {
                self.exhausted = true;
                if root {
                    node.value = Value::Opaque { shape: node.value.shape(), reason: Unindexed::RowBudget };
                    encoded = super::codec::encode(&node);
                }
            }
            if root || !self.exhausted {
                if encoded.len() > super::NODE_BYTES {
                    return Err(io::Error::other("JSON descriptor exceeds its bound"));
                }
                address = Some(Address(ByteCount(nodes.bytes()?)));
                nodes.push(&(encoded.len() as u32).to_be_bytes(), sink).await?;
                nodes.push(&encoded, sink).await?;
            }
        }
        if let Some(parent) = self.stack.last_mut() {
            if let Some(address) = address {
                parent.last = Some(address);
            }
            parent.count.0 =
                parent.count.0.checked_add(1).ok_or_else(|| io::Error::other("JSON member count overflow"))?;
        } else {
            self.root = address;
        }
        Ok(())
    }
    pub async fn event(
        &mut self,
        event: Event<'_>,
        through: ByteCount,
        strings: &mut BufferedBody,
        nodes: &mut BufferedBody,
        sink: &mut dyn BodySink,
    ) -> io::Result<()> {
        match event {
            Event::BeginObject | Event::BeginArray => {
                let (place, selection) = self.place()?;
                let object = matches!(event, Event::BeginObject);
                let selection =
                    if place.capture && !self.exhausted { selection.container(object) } else { Selection::Opaque };
                self.stack.push(Frame {
                    raw_start: back(through, 1)?,
                    object,
                    place,
                    selection,
                    key: None,
                    last: None,
                    count: Count(0),
                });
            }
            Event::EndObject | Event::EndArray => {
                let frame = self.stack.pop().ok_or_else(|| io::Error::other("JSON stack underflow"))?;
                let value = if frame.selection == Selection::Opaque {
                    Value::Opaque {
                        shape: if frame.object { Shape::Object } else { Shape::Array },
                        reason: Unindexed::NotSelected,
                    }
                } else if frame.object {
                    Value::Object { last: frame.last, members: frame.count }
                } else {
                    Value::Array { last: frame.last, members: frame.count }
                };
                self.node(frame.place, value, extent(frame.raw_start, through)?, nodes, sink).await?;
            }
            Event::BeginString { key } => {
                let (place, store) = if key {
                    (None, self.stack.last().is_some_and(|p| p.selection != Selection::Opaque) && !self.exhausted)
                } else {
                    let (place, selection) = self.place()?;
                    let store = place.capture && selection != Selection::Opaque && !self.exhausted;
                    (Some(place), store)
                };
                self.scalar = Some(Scalar {
                    raw_start: back(through, 1)?,
                    start: ByteCount(strings.bytes()?),
                    key,
                    number: false,
                    store,
                    small: (key && store).then(Vec::new),
                    place,
                });
            }
            Event::BeginNumber => {
                let (place, selection) = self.place()?;
                let store = place.capture && selection != Selection::Opaque && !self.exhausted;
                self.scalar = Some(Scalar {
                    raw_start: through,
                    start: ByteCount(strings.bytes()?),
                    key: false,
                    number: true,
                    store,
                    small: None,
                    place: Some(place),
                });
            }
            Event::StringBytes(bytes) | Event::NumberBytes(bytes) => self.bytes(bytes, strings, sink).await?,
            Event::Escaped { bytes, len } => self.bytes(&bytes[..len as usize], strings, sink).await?,
            Event::EndString { .. } | Event::EndNumber => {
                let scalar = self.scalar.take().ok_or_else(|| io::Error::other("JSON scalar has no start"))?;
                let text = Span { offset: scalar.start, bytes: ByteCount(strings.bytes()? - scalar.start.0) };
                if scalar.key {
                    let small = scalar.small.map(String::from_utf8).transpose().map_err(io::Error::other)?;
                    self.stack.last_mut().ok_or_else(|| io::Error::other("JSON key outside object"))?.key =
                        Some(Key { span: text, small });
                } else {
                    let value = if !scalar.store {
                        Value::Opaque {
                            shape: if scalar.number { Shape::Number } else { Shape::String },
                            reason: Unindexed::NotSelected,
                        }
                    } else if scalar.number {
                        Value::Number { text }
                    } else {
                        Value::String { text }
                    };
                    let place = scalar.place.ok_or_else(|| io::Error::other("JSON scalar has no position"))?;
                    self.node(place, value, extent(scalar.raw_start, through)?, nodes, sink).await?;
                }
            }
            Event::Bool(value) => {
                let (place, selection) = self.place()?;
                let shape = if selection == Selection::Opaque {
                    Value::Opaque { shape: Shape::Bool, reason: Unindexed::NotSelected }
                } else {
                    Value::Bool { value }
                };
                self.node(place, shape, extent(back(through, if value { 4 } else { 5 })?, through)?, nodes, sink)
                    .await?;
            }
            Event::Null => {
                let (place, _) = self.place()?;
                self.node(place, Value::Null, extent(back(through, 4)?, through)?, nodes, sink).await?;
            }
        }
        Ok(())
    }
    async fn bytes(&mut self, bytes: &[u8], strings: &mut BufferedBody, sink: &mut dyn BodySink) -> io::Result<()> {
        let scalar = self.scalar.as_mut().ok_or_else(|| io::Error::other("JSON bytes outside scalar"))?;
        if scalar.store {
            strings.push(bytes, sink).await?;
        }
        if let Some(small) = &mut scalar.small {
            if small.len() + bytes.len() <= 64 {
                small.extend_from_slice(bytes);
            } else {
                scalar.small = None;
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
