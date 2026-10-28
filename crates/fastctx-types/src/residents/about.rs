//! Compatibility for records written before a conversation carried its account.

use super::{About, Host, Origin};
use crate::{Address, MachineName, ResidentId};
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct Stored {
    id: ResidentId,
    host: Host,
    machine: MachineName,
    born_at: jiff::Timestamp,
    origin: Origin,
    #[serde(default)]
    forked_from: Option<Address>,
    session: Option<String>,
    #[serde(default)]
    world_started: bool,
    #[serde(default)]
    native_resumed: bool,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    acting: Option<crate::access::Acting>,
    #[serde(default)]
    focus: crate::focus::Focus,
    #[serde(default)]
    mcp_off: std::collections::BTreeSet<String>,
    #[serde(default)]
    home: Option<std::path::PathBuf>,
    #[serde(default)]
    callee: Option<String>,
}

impl TryFrom<Stored> for About {
    type Error = &'static str;

    fn try_from(stored: Stored) -> Result<Self, Self::Error> {
        if stored.native_resumed && stored.host.agent.as_str() != "codex" {
            return Err("native resumption belongs to a Codex thread");
        }
        let acting = match stored.acting {
            Some(acting) => acting,
            None if stored.origin == Origin::User => crate::access::Acting::Here,
            None => return Err("a colleague's record has no account identity"),
        };
        Ok(Self {
            id: stored.id,
            host: stored.host,
            machine: stored.machine,
            born_at: stored.born_at,
            origin: stored.origin,
            forked_from: stored.forked_from,
            session: stored.session,
            world_started: stored.world_started,
            native_resumed: stored.native_resumed,
            model: stored.model,
            acting,
            focus: stored.focus,
            mcp_off: stored.mcp_off,
            home: stored.home,
            callee: stored.callee,
        })
    }
}
