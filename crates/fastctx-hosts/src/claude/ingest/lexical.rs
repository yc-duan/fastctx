//! Restartable JSON syntax validation. No string, number, array or object is
//! collected here. Borrowed fragments are at most one 64 KiB input window.

mod scalars;
mod strings;
use scalars::{Literal, Number};
use serde::{Deserialize, Serialize};
use strings::StringState;

pub const WINDOW_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub offset: usize,
    pub message: &'static str,
}
impl Error {
    fn at(offset: usize, message: &'static str) -> Self {
        Self { offset, message }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{} at input byte {}", self.message, self.offset)
    }
}
impl std::error::Error for Error {}

#[derive(Debug, PartialEq, Eq)]
pub enum Event<'a> {
    BeginObject,
    EndObject,
    BeginArray,
    EndArray,
    BeginString { key: bool },
    StringBytes(&'a [u8]),
    Escaped { bytes: [u8; 4], len: u8 },
    EndString { key: bool },
    BeginNumber,
    NumberBytes(&'a [u8]),
    EndNumber,
    Bool(bool),
    Null,
}
#[derive(Debug)]
pub struct Step<'a> {
    pub used: usize,
    pub event: Option<Event<'a>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Object {
    First,
    Key,
    Colon,
    Value,
    After,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Array {
    First,
    Value,
    After,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Frame {
    Object(Object),
    Array(Array),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Mode {
    Idle,
    String { key: bool, state: StringState },
    Number(Number),
    Literal { kind: Literal, read: u8 },
}

/// This bounded state is saved beside staged body roots. A complete syntax tree
/// is still provisional until the framing layer validates LF or a stable seal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lexer {
    depth_limit: u32,
    stack: Vec<Frame>,
    mode: Mode,
    started: bool,
    complete: bool,
}
impl Lexer {
    pub fn new(depth_limit: u32) -> Self {
        Self { depth_limit, stack: Vec::new(), mode: Mode::Idle, started: false, complete: false }
    }
    pub fn is_empty(&self) -> bool {
        !self.started
    }
    pub fn is_complete(&self) -> bool {
        self.complete && self.stack.is_empty() && self.mode == Mode::Idle
    }
    pub fn validate_checkpoint(&self) -> Result<(), Error> {
        if self.depth_limit == 0
            || self.stack.len() > self.depth_limit as usize
            || self.complete && (!self.started || !self.stack.is_empty() || self.mode != Mode::Idle)
        {
            return Err(Error::at(0, "invalid saved JSON state"));
        }
        Ok(())
    }
    fn start_value(&mut self, offset: usize) -> Result<(), Error> {
        match self.stack.last_mut() {
            Some(Frame::Object(stage @ Object::Value)) => *stage = Object::After,
            Some(Frame::Array(stage @ (Array::First | Array::Value))) => *stage = Array::After,
            None if !self.started => self.started = true,
            _ => return Err(Error::at(offset, "unexpected JSON value")),
        }
        Ok(())
    }
    fn end_value(&mut self) {
        if self.stack.is_empty() {
            self.complete = true;
        }
    }

    /// Zero consumed bytes are allowed only when an event closes or opens a
    /// token. The next call makes progress; callers must consume that event.
    pub fn step<'a>(&mut self, input: &'a [u8]) -> Result<Step<'a>, Error> {
        let input = &input[..input.len().min(WINDOW_BYTES)];
        let mut used = 0;
        loop {
            match self.mode {
                Mode::String { key, mut state } => {
                    let step = state.step(&input[used..], key).map_err(|e| Error::at(used + e.offset, e.message))?;
                    self.mode = Mode::String { key, state };
                    if matches!(step.event, Some(Event::EndString { .. })) {
                        self.mode = Mode::Idle;
                        if key {
                            match self.stack.last_mut() {
                                Some(Frame::Object(stage @ (Object::First | Object::Key))) => *stage = Object::Colon,
                                _ => return Err(Error::at(used, "key outside object")),
                            }
                        } else {
                            self.end_value();
                        }
                    }
                    return Ok(Step { used: used + step.used, event: step.event });
                }
                Mode::Number(mut number) => {
                    let step = number.step(&input[used..]).map_err(|e| Error::at(used + e.offset, e.message))?;
                    self.mode = Mode::Number(number);
                    if matches!(step.event, Some(Event::EndNumber)) {
                        self.mode = Mode::Idle;
                        self.end_value();
                    }
                    return Ok(Step { used: used + step.used, event: step.event });
                }
                Mode::Literal { kind, mut read } => {
                    let step =
                        kind.step(&mut read, &input[used..]).map_err(|e| Error::at(used + e.offset, e.message))?;
                    self.mode = Mode::Literal { kind, read };
                    if step.event.is_some() {
                        self.mode = Mode::Idle;
                        self.end_value();
                    }
                    return Ok(Step { used: used + step.used, event: step.event });
                }
                Mode::Idle => {}
            }
            let Some(&byte) = input.get(used) else {
                return Ok(Step { used, event: None });
            };
            if matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
                used += 1;
                continue;
            }
            if self.complete {
                return Err(Error::at(used, "trailing bytes after JSON value"));
            }
            match byte {
                b'{' | b'[' => {
                    self.start_value(used)?;
                    if self.stack.len() >= self.depth_limit as usize {
                        return Err(Error::at(used, "JSON nesting exceeds the decoder budget"));
                    }
                    self.stack.push(if byte == b'{' {
                        Frame::Object(Object::First)
                    } else {
                        Frame::Array(Array::First)
                    });
                    return Ok(Step {
                        used: used + 1,
                        event: Some(if byte == b'{' { Event::BeginObject } else { Event::BeginArray }),
                    });
                }
                b'}' | b']' => {
                    if !matches!(
                        (byte, self.stack.last()),
                        (b'}', Some(Frame::Object(Object::First | Object::After)))
                            | (b']', Some(Frame::Array(Array::First | Array::After)))
                    ) {
                        return Err(Error::at(used, "unexpected closing delimiter"));
                    }
                    self.stack.pop();
                    self.end_value();
                    return Ok(Step {
                        used: used + 1,
                        event: Some(if byte == b'}' { Event::EndObject } else { Event::EndArray }),
                    });
                }
                b',' => {
                    match self.stack.last_mut() {
                        Some(Frame::Object(stage @ Object::After)) => *stage = Object::Key,
                        Some(Frame::Array(stage @ Array::After)) => *stage = Array::Value,
                        _ => return Err(Error::at(used, "unexpected comma")),
                    };
                    used += 1;
                }
                b':' => {
                    match self.stack.last_mut() {
                        Some(Frame::Object(stage @ Object::Colon)) => *stage = Object::Value,
                        _ => return Err(Error::at(used, "unexpected colon")),
                    };
                    used += 1;
                }
                b'"' => {
                    let key = matches!(self.stack.last(), Some(Frame::Object(Object::First | Object::Key)));
                    if !key {
                        self.start_value(used)?;
                    }
                    self.mode = Mode::String { key, state: StringState::default() };
                    return Ok(Step { used: used + 1, event: Some(Event::BeginString { key }) });
                }
                b'-' | b'0'..=b'9' => {
                    self.start_value(used)?;
                    self.mode = Mode::Number(Number::Start);
                    return Ok(Step { used, event: Some(Event::BeginNumber) });
                }
                b't' | b'f' | b'n' => {
                    self.start_value(used)?;
                    self.mode = Mode::Literal {
                        kind: match byte {
                            b't' => Literal::True,
                            b'f' => Literal::False,
                            _ => Literal::Null,
                        },
                        read: 0,
                    };
                }
                _ => return Err(Error::at(used, "unexpected JSON byte")),
            }
        }
    }

    /// Called only at LF or after an authorized stable source seal. Ordinary
    /// EOF never calls this: its state and staged bytes remain pending.
    pub fn finish(&mut self) -> Result<Option<Event<'static>>, Error> {
        let event = if let Mode::Number(number) = self.mode {
            if !number.complete() {
                return Err(Error::at(0, "incomplete JSON number"));
            }
            self.mode = Mode::Idle;
            self.end_value();
            Some(Event::EndNumber)
        } else {
            None
        };
        if !self.is_complete() {
            return Err(Error::at(0, "incomplete JSON value"));
        }
        Ok(event)
    }
}
