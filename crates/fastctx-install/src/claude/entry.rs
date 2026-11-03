//! Fastctx's MCP server entered in Claude Code's global state, for a login
//! the workbench is not open in (`world-layers.md`, 接入只写接入的东西):
//! `mcpServers.fastctx` in its `.claude.json`, the way `claude mcp add
//! --scope user` enters one. A login the workbench is open in has the same
//! server from the World's plugin instead, never both, or its sessions would
//! see `run` twice.
//!
//! Like the other engines here, these work on bytes the caller read and
//! hand back bytes for the caller to publish, under Claude Code's own lock.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agents::json::{self, Placed};
use crate::{Error, Notice};

/// The entry is `mcpServers.fastctx`.
const AT: [&str; 1] = ["mcpServers"];
const NAME: &str = "fastctx";

/// What was entered, and in the state of which login.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    /// The value of `CLAUDE_CONFIG_DIR` the state was found by; none where
    /// it was unset.
    configured: Option<PathBuf>,
    placed: Placed,
}

impl Receipt {
    pub fn configured(&self) -> Option<&OsStr> {
        self.configured.as_deref().map(Path::as_os_str)
    }
}

/// The entry for the server `command` that serves fastctx's directory
/// `root`, for the login `configured` names: as the World's plugin starts
/// the same server, which reads the login from `FASTCTX_AGENT_HOME`.
pub fn wanted(command: &str, root: &str, configured: Option<&OsStr>) -> Value {
    json!({
        "type": "stdio",
        "command": command,
        "args": ["serve"],
        "env": {
            "FASTCTX_HOME": root,
            "FASTCTX_AGENT_HOME": configured.map(|dir| dir.to_string_lossy().into_owned()).unwrap_or_default(),
        },
    })
}

/// The state's bytes with the entry in, `None` where it is in already as
/// `wanted`, and the receipt. `previous`, the receipt of an earlier entry,
/// lets one still as written be written anew; one changed since, or an
/// entry of that name Fastctx did not make, is refused unless `take_back`.
/// An entry with no receipt that is exactly what is wanted was left by a
/// change that stopped before its receipt was kept: it is Fastctx's.
pub fn place(
    bytes: Option<&[u8]>,
    location: &str,
    wanted: &Value,
    configured: Option<&OsStr>,
    previous: Option<&Receipt>,
    take_back: bool,
) -> Result<(Option<Vec<u8>>, Receipt), Error> {
    let left = previous.is_none() && current(bytes, location)?.as_ref() == Some(wanted);
    let (next, placed) =
        json::place(bytes, location, &AT, NAME, wanted, previous.map(|receipt| &receipt.placed), take_back, false)?;
    let placed = if left { placed.owned() } else { placed };
    let receipt = Receipt { configured: configured.map(PathBuf::from), placed };
    Ok(((Some(next.as_slice()) != bytes).then_some(next), receipt))
}

/// The state's bytes with the entry taken out while it is still as written,
/// `None` where there is nothing to change, and why it was left where the
/// user changed it since. The objects Fastctx made on the way go with it; a
/// state file is never removed, since Claude Code keeps its own in it.
pub fn unplace(
    bytes: Option<&[u8]>,
    location: &str,
    receipt: &Receipt,
) -> Result<(Option<Vec<u8>>, Option<Notice>), Error> {
    let Some(bytes) = bytes else { return Ok((None, None)) };
    let (next, notice) = json::unplace(Some(bytes), location, &receipt.placed)?;
    let next = next.unwrap_or_else(|| b"{}\n".to_vec());
    Ok(((next != bytes).then_some(next), notice))
}

/// The entry as written by the change `receipt` records: what to put back
/// when the change it was taken out with did not go through.
pub fn written(receipt: &Receipt) -> Value {
    receipt.placed.written().clone()
}

/// The entry as the state holds it now, if any.
pub fn current(bytes: Option<&[u8]>, location: &str) -> Result<Option<Value>, Error> {
    let Some(bytes) = bytes else { return Ok(None) };
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let options =
        jsonc_parser::ParseOptions { allow_comments: true, allow_trailing_commas: true, ..Default::default() };
    let root = jsonc_parser::parse_to_serde_value::<Option<Value>>(text, &options)
        // The parser's message can hold the file's own text: it is not told.
        .map_err(|_| Error::at(location, "is not valid JSON"))?
        .unwrap_or_default();
    Ok(root.get(AT[0]).and_then(|servers| servers.get(NAME)).cloned())
}

/// Whether a key path of the state is the entry's, whose values may be shown.
pub fn shown(path: &[String]) -> bool {
    path.first().is_some_and(|key| key == AT[0]) && path.get(1).is_some_and(|key| key == NAME)
}
