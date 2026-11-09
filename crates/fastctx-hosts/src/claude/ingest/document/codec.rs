//! Compact, private node descriptors. Varints avoid JSON field-name amplification;
//! the builder independently enforces a whole-row byte budget.
use super::{Address, Node, Shape, Span, Unindexed, Value};
use fastctx_types::sync::cursor::{ByteCount, Count};
use std::io;

pub(super) fn encode(node: &Node) -> Vec<u8> {
    let mut out = vec![1];
    span(&mut out, node.raw);
    out.push(u8::from(node.name.is_some()));
    if let Some(name) = node.name {
        span(&mut out, name);
    }
    uint(&mut out, node.ordinal.0);
    address(&mut out, node.previous);
    match node.value {
        Value::Object { last, members } | Value::Array { last, members } => {
            out.push(if matches!(node.value, Value::Object { .. }) { 0 } else { 1 });
            address(&mut out, last);
            uint(&mut out, members.0);
        }
        Value::String { text } | Value::Number { text } => {
            out.push(if matches!(node.value, Value::String { .. }) { 2 } else { 3 });
            span(&mut out, text);
        }
        Value::Bool { value } => out.push(if value { 4 } else { 5 }),
        Value::Null => out.push(6),
        Value::Opaque { shape, reason } => {
            out.push(7);
            out.push(match shape {
                Shape::Object => 0,
                Shape::Array => 1,
                Shape::String => 2,
                Shape::Number => 3,
                Shape::Bool => 4,
                Shape::Null => 5,
            });
            out.push(match reason {
                Unindexed::NotSelected => 0,
                Unindexed::RowBudget => 1,
            });
        }
    }
    out
}
fn uint(out: &mut Vec<u8>, mut value: u64) {
    while value > 127 {
        out.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    out.push(value as u8);
}
fn span(out: &mut Vec<u8>, value: Span) {
    uint(out, value.offset.0);
    uint(out, value.bytes.0);
}
fn address(out: &mut Vec<u8>, value: Option<Address>) {
    out.push(u8::from(value.is_some()));
    if let Some(value) = value {
        uint(out, value.0.0);
    }
}
struct Input<'a>(&'a [u8]);
impl Input<'_> {
    fn byte(&mut self) -> io::Result<u8> {
        let (byte, rest) = self.0.split_first().ok_or_else(invalid)?;
        self.0 = rest;
        Ok(*byte)
    }
    fn uint(&mut self) -> io::Result<u64> {
        let mut value = 0;
        for shift in (0..70).step_by(7) {
            let byte = self.byte()?;
            if shift == 63 && byte > 1 {
                return Err(invalid());
            }
            value |= u64::from(byte & 127) << shift;
            if byte < 128 {
                if shift > 0 && byte == 0 {
                    return Err(invalid());
                }
                return Ok(value);
            }
        }
        Err(invalid())
    }
    fn span(&mut self) -> io::Result<Span> {
        Ok(Span { offset: ByteCount(self.uint()?), bytes: ByteCount(self.uint()?) })
    }
    fn address(&mut self) -> io::Result<Option<Address>> {
        match self.byte()? {
            0 => Ok(None),
            1 => Ok(Some(Address(ByteCount(self.uint()?)))),
            _ => Err(invalid()),
        }
    }
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid private JSON descriptor")
}
pub(super) fn decode(bytes: &[u8]) -> io::Result<Node> {
    let mut input = Input(bytes);
    if input.byte()? != 1 {
        return Err(invalid());
    }
    let raw = input.span()?;
    let name = match input.byte()? {
        0 => None,
        1 => Some(input.span()?),
        _ => return Err(invalid()),
    };
    let ordinal = Count(input.uint()?);
    let previous = input.address()?;
    let value = match input.byte()? {
        0 => Value::Object { last: input.address()?, members: Count(input.uint()?) },
        1 => Value::Array { last: input.address()?, members: Count(input.uint()?) },
        2 => Value::String { text: input.span()? },
        3 => Value::Number { text: input.span()? },
        4 => Value::Bool { value: true },
        5 => Value::Bool { value: false },
        6 => Value::Null,
        7 => Value::Opaque {
            shape: match input.byte()? {
                0 => Shape::Object,
                1 => Shape::Array,
                2 => Shape::String,
                3 => Shape::Number,
                4 => Shape::Bool,
                5 => Shape::Null,
                _ => return Err(invalid()),
            },
            reason: match input.byte()? {
                0 => Unindexed::NotSelected,
                1 => Unindexed::RowBudget,
                _ => return Err(invalid()),
            },
        },
        _ => return Err(invalid()),
    };
    if !input.0.is_empty() {
        return Err(invalid());
    }
    Ok(Node { raw, name, ordinal, previous, value })
}
