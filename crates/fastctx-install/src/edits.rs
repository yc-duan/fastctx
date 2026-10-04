//! What the engines that edit the hosts' files share: a file's bytes as
//! read, a place left alone and why, and why an edit was refused. No engine
//! reads or writes a file itself; the caller reads, and the transaction
//! writes.

use serde::{Deserialize, Serialize};

/// A caller-selected file. `None` means the file does not exist; an empty
/// slice means an existing empty file. No function in this crate performs IO.
/// Paths are stable identity strings compared exactly against receipts;
/// resolving filesystem aliases and binding bytes to paths belongs to the caller.
#[derive(Clone, Copy, Debug)]
pub struct File<'a> {
    pub path: &'a str,
    pub bytes: Option<&'a [u8]>,
}

/// A place left untouched, or a limitation inherited from an old receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub location: String,
    pub reason: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("{location}: {reason}")]
    Conflict { location: String, reason: String },
    #[error("{location}: tool_output_token_limit is {current}; raising it to {requested} requires approval")]
    TokenLimitApproval { location: String, current: i64, requested: u64 },
}

impl Error {
    pub(crate) fn at(location: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Conflict { location: location.into(), reason: reason.into() }
    }
}

pub(crate) fn check_path(actual: &str, recorded: &str) -> Result<(), Error> {
    if actual != recorded {
        return Err(Error::at(actual, format!("the receipt belongs to {recorded}")));
    }
    Ok(())
}

pub(crate) fn finish(bytes: Vec<u8>, existed: bool) -> Option<Vec<u8>> {
    (existed || !bytes.is_empty()).then_some(bytes)
}
