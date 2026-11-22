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
    #[serde(default)]
    side: bool,
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
    idle_compaction: Option<bool>,
    #[serde(default)]
    home: Option<std::path::PathBuf>,
    #[serde(default)]
    login: Option<crate::logins::LoginId>,
    #[serde(default)]
    born_as: Option<Vec<super::BornAs>>,
    #[serde(default)]
    callee: Option<String>,
    #[serde(default)]
    born_shared: std::collections::BTreeMap<String, Option<String>>,
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
            side: stored.side,
            session: stored.session,
            world_started: stored.world_started,
            native_resumed: stored.native_resumed,
            model: stored.model,
            acting,
            focus: stored.focus,
            mcp_off: stored.mcp_off,
            idle_compaction: stored.idle_compaction,
            home: stored.home,
            login: stored.login,
            born_as: stored.born_as,
            callee: stored.callee,
            born_shared: stored.born_shared,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::About;

    fn about(login: Option<&str>) -> serde_json::Value {
        let mut about = serde_json::json!({
            "id": "claude-ab12", "host": {"agent": "claude", "client": "claude-code", "version": ""},
            "machine": "laptop", "born_at": "2026-11-20T00:00:00Z", "origin": {"kind": "user"}, "session": null,
        });
        if let Some(login) = login {
            about["login"] = login.into();
        }
        about
    }

    #[test]
    fn a_login_is_kept_where_it_was_born_and_none_before() {
        let before: About = serde_json::from_value(about(None)).expect("a record from before");
        assert_eq!(before.login, None);
        assert!(serde_json::to_value(&before).expect("written").get("login").is_none());
        let born: About = serde_json::from_value(about(Some("l-0011223344556677"))).expect("a record");
        assert_eq!(born.login.as_ref().map(|login| login.as_str()), Some("l-0011223344556677"));
        let again: About = serde_json::from_value(serde_json::to_value(&born).expect("written")).expect("read again");
        assert_eq!(again, born);
        assert!(serde_json::from_value::<About>(about(Some("/home/ada/.claude"))).is_err());
    }
}
