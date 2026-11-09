//! Logical coordinates survive byte-tree splits without searching by content hash.
use crate::{manifest::PathName, store::sql};
use fastctx_storage::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(super) enum Part {
    Name(Name),
    Range { first: String, last: String },
    Metadata { codec: String, index: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(super) enum Name {
    Text(String),
    Unix(Vec<u8>),
    Windows(Vec<u16>),
}
impl From<&PathName> for Name {
    fn from(value: &PathName) -> Self {
        match value {
            PathName::Utf8(value) => Self::Text(value.clone()),
            PathName::UnixBytes(value) => {
                String::from_utf8(value.clone()).map(Self::Text).unwrap_or_else(|_| Self::Unix(value.clone()))
            }
            PathName::WindowsUnits(value) => {
                String::from_utf16(value).map(Self::Text).unwrap_or_else(|_| Self::Windows(value.clone()))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Span {
    pub start: u64,
    pub end: u64,
}
impl Span {
    pub fn new(start: u64, end: u64) -> Result<Self> {
        if end < start {
            return Err(sql::invalid("supply byte interval runs backwards"));
        }
        Ok(Self { start, end })
    }
    fn contains(self, other: Self) -> bool {
        self.start <= other.start && self.end >= other.end
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Address {
    trail: Vec<Part>,
    pub span: Option<Span>,
}
impl Address {
    pub fn path(&self, names: &[PathName]) -> Result<Self> {
        if self.span.is_some() || names.is_empty() {
            return Err(sql::invalid("a directory path cannot be nested in a byte interval"));
        }
        let mut next = self.clone();
        next.trail.extend(names.iter().map(|name| Part::Name(name.into())));
        next.bound()?;
        Ok(next)
    }
    pub fn bytes(&self, span: Span) -> Result<Self> {
        if self.span.is_some_and(|parent| !parent.contains(span)) {
            return Err(sql::invalid("supply child escapes its declared byte interval"));
        }
        Ok(Self { trail: self.trail.clone(), span: Some(span) })
    }
    pub(super) fn structural(&self, part: Part) -> Result<Self> {
        if self.span.is_some() {
            return Err(sql::invalid("a byte-tree interval points to a different manifest shape"));
        }
        let mut next = self.clone();
        next.trail.push(part);
        next.bound()?;
        Ok(next)
    }
    pub fn resolve(&self, extent: Option<Span>) -> Result<Self> {
        match (self.span, extent) {
            (Some(expected), Some(actual)) if expected != actual => {
                Err(sql::invalid("byte-tree page differs from its parent interval"))
            }
            (Some(_), None) => Err(sql::invalid("byte-tree interval has a non-byte child")),
            (_, extent) => Ok(Self { trail: self.trail.clone(), span: extent }),
        }
    }
    pub fn contains(&self, target: &Self) -> bool {
        if !target.trail.starts_with(&self.trail) {
            return false;
        }
        match (self.span, target.span) {
            (None, _) => true,
            (Some(left), Some(right)) => self.trail == target.trail && left.contains(right),
            (Some(_), None) => false,
        }
    }
    pub fn join(&self, child: &Self) -> Result<Self> {
        if child.trail.is_empty() {
            return match child.span {
                Some(span) => self.bytes(span),
                None => Ok(self.clone()),
            };
        }
        if self.span.is_some() {
            return Err(sql::invalid("structural child is nested in a byte interval"));
        }
        let mut next = self.clone();
        next.trail.extend_from_slice(&child.trail);
        next.span = child.span;
        next.bound()?;
        Ok(next)
    }
    pub fn same_tree(&self, other: &Self) -> bool {
        self.trail == other.trail
    }
    fn bound(&self) -> Result<()> {
        if self.trail.len() > 128
            || serde_json::to_vec(&self.trail).map_err(|e| sql::invalid(e.to_string()))?.len() > 16 * 1024
        {
            return Err(sql::invalid("supply position exceeds its path budget"));
        }
        Ok(())
    }
}
