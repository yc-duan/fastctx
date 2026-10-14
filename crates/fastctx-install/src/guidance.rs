//! The same byte-level marker contract serves AGENTS.md and CLAUDE.md.

use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::{Error, File, Notice, check_path, finish};

mod shared;

pub const BEGIN: &[u8] = b"<!-- fastctx:begin -->";
pub const END: &[u8] = b"<!-- fastctx:end -->";
pub(crate) const OLD_BEGIN: &[u8] = b"<!-- fastread:begin -->";
pub(crate) const OLD_END: &[u8] = b"<!-- fastread:end -->";

/// Persist this together with the returned bytes. It records only owned
/// bytes, never a backup of the user's surrounding instructions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub(crate) path: String,
    pub(crate) original_existed: bool,
    pub(crate) block: Vec<u8>,
    pub(crate) prefix: Vec<u8>,
    pub(crate) suffix: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Applied {
    pub bytes: Vec<u8>,
    pub receipt: Receipt,
}

#[derive(Clone, Debug)]
pub struct Unapplied {
    /// `None` requests deletion. An existing empty user file stays `Some`.
    pub bytes: Option<Vec<u8>>,
    /// Keep a remaining receipt if the user later wants to resolve drift.
    pub receipt: Option<Receipt>,
    pub retained: Vec<Notice>,
}

/// Inserts the supplied body verbatim between marker lines. Existing blocks
/// need a matching receipt and identical bytes. The body may not contain a
/// marker. The surrounding file need not be UTF-8.
pub fn apply(file: File<'_>, body: &str, previous: Option<&Receipt>) -> Result<Applied, Error> {
    let block = block(body, file.path)?;
    let original = file.bytes.unwrap_or_default();
    let range = marker_range(original, BEGIN, END, file.path)?;
    if let Some(receipt) = previous
        && let Some(range) = range
    {
        check_path(file.path, &receipt.path)?;
        if original[range.clone()] != receipt.block {
            return Err(Error::at(file.path, "the fastctx block differs from the recorded bytes"));
        }
        let mut bytes = original.to_vec();
        bytes.splice(range, block.iter().copied());
        let mut receipt = receipt.clone();
        receipt.block = block;
        return Ok(Applied { bytes, receipt });
    }
    if range.is_some() {
        return Err(Error::at(file.path, "the fastctx block has no ownership receipt"));
    }
    // A recorded block that is gone goes back in; original file existence
    // still comes from the first apply, rather than a later synchronization.
    if let Some(receipt) = previous {
        check_path(file.path, &receipt.path)?;
    }
    let prefix = separator(original);
    let suffix = b"\n".to_vec();
    let mut bytes = original.to_vec();
    bytes.extend_from_slice(&prefix);
    bytes.extend_from_slice(&block);
    bytes.extend_from_slice(&suffix);
    let original_existed = previous.map_or(file.bytes.is_some(), |receipt| receipt.original_existed);
    Ok(Applied { bytes, receipt: Receipt { path: file.path.into(), original_existed, block, prefix, suffix } })
}

/// `apply`, rewriting a recorded block the user changed since as `body`
/// says; what lies around it is theirs and stays.
pub fn take_back(file: File<'_>, body: &str, previous: Option<&Receipt>) -> Result<Applied, Error> {
    let original = file.bytes.unwrap_or_default();
    if let Some(receipt) = previous
        && let Some(range) = marker_range(original, BEGIN, END, file.path)?
        && original[range.clone()] != receipt.block
    {
        check_path(file.path, &receipt.path)?;
        let block = block(body, file.path)?;
        let mut bytes = original.to_vec();
        bytes.splice(range, block.iter().copied());
        let mut receipt = receipt.clone();
        receipt.block = block;
        return Ok(Applied { bytes, receipt });
    }
    apply(file, body, previous)
}

pub(crate) fn apply_shared(file: File<'_>, body: &str, previous: Option<&Receipt>) -> Result<(Applied, bool), Error> {
    shared::apply(file, body, previous)
}

fn block(body: &str, location: &str) -> Result<Vec<u8>, Error> {
    for marker in [BEGIN, END, OLD_BEGIN, OLD_END] {
        if contains(body.as_bytes(), marker) {
            return Err(Error::at(location, "the supplied guidance body contains a reserved marker"));
        }
    }
    let mut block = BEGIN.to_vec();
    block.push(b'\n');
    block.extend_from_slice(body.as_bytes());
    if !body.ends_with('\n') {
        block.push(b'\n');
    }
    block.extend_from_slice(END);
    Ok(block)
}

/// Removes only a block whose bytes still match. Drift is returned as a
/// notice, with the file and its receipt intact. With no receipt this is an
/// identity operation, including on malformed or non-UTF-8 input.
pub fn unapply(file: File<'_>, receipt: Option<&Receipt>) -> Result<Unapplied, Error> {
    let Some(receipt) = receipt else {
        return Ok(Unapplied { bytes: file.bytes.map(<[u8]>::to_vec), receipt: None, retained: vec![] });
    };
    check_path(file.path, &receipt.path)?;
    let Some(original) = file.bytes else {
        return Ok(Unapplied { bytes: None, receipt: None, retained: vec![] });
    };
    let mut retained = vec![];
    let range = match marker_range(original, BEGIN, END, file.path) {
        Ok(None) => {
            return Ok(Unapplied { bytes: Some(original.to_vec()), receipt: None, retained });
        }
        Ok(Some(range)) if original[range.clone()] == receipt.block => range,
        result => {
            let reason = match result {
                Err(error) => error.to_string(),
                _ => "the fastctx block differs from the recorded bytes".into(),
            };
            retained.push(Notice { location: file.path.into(), reason });
            return Ok(Unapplied { bytes: Some(original.to_vec()), receipt: Some(receipt.clone()), retained });
        }
    };
    let mut start = range.start;
    let mut end = range.end;
    if original[..start].ends_with(&receipt.prefix) {
        start -= receipt.prefix.len();
    } else if !receipt.prefix.is_empty() {
        retained.push(Notice {
            location: file.path.into(),
            reason: "the separator before the block was changed; left it in place".into(),
        });
    }
    if original[end..].starts_with(&receipt.suffix) {
        end += receipt.suffix.len();
    } else if !receipt.suffix.is_empty() {
        retained.push(Notice {
            location: file.path.into(),
            reason: "the newline after the block was changed; left it in place".into(),
        });
    }
    let mut bytes = original.to_vec();
    bytes.drain(start..end);
    Ok(Unapplied { bytes: finish(bytes, receipt.original_existed), receipt: None, retained })
}

pub(crate) fn separator(bytes: &[u8]) -> Vec<u8> {
    if bytes.is_empty() || bytes.ends_with(b"\n\n") || bytes.ends_with(b"\r\n\r\n") {
        vec![]
    } else if bytes.ends_with(b"\r\n") {
        b"\r\n".to_vec()
    } else if bytes.ends_with(b"\n") {
        b"\n".to_vec()
    } else {
        b"\n\n".to_vec()
    }
}

fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|part| part == needle)
}

pub(crate) fn marker_range(
    bytes: &[u8],
    begin: &[u8],
    end: &[u8],
    location: &str,
) -> Result<Option<Range<usize>>, Error> {
    let starts: Vec<_> =
        bytes.windows(begin.len()).enumerate().filter_map(|(i, p)| (p == begin).then_some(i)).collect();
    let ends: Vec<_> = bytes.windows(end.len()).enumerate().filter_map(|(i, p)| (p == end).then_some(i)).collect();
    match (starts.as_slice(), ends.as_slice()) {
        ([], []) => Ok(None),
        ([start], [stop]) if start < stop => Ok(Some(*start..*stop + end.len())),
        _ => Err(Error::at(
            location,
            format!("duplicate, unmatched, or reversed {} markers", String::from_utf8_lossy(begin)),
        )),
    }
}
