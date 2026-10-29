//! Whether a session the World did not start is its machine owner's own
//! (`world-network.md`, 共享机器与智能体协助).
//!
//! A Claude session the user opens reaches the World through its plugin, a
//! Codex thread through the World's tools, and the World takes either as
//! the owner's: it works for this machine's own accounts. Under a login that
//! assists other accounts that holds only for what the user's own program
//! runs, a terminal, an app or an editor. A session that Claude Code's
//! daemon or the World's own Codex host runs there and the World does not
//! know may be anyone's, as a copy Claude Code makes of a conversation is,
//! or a thread a Codex conversation spawns: taken as the owner's, its
//! history would show to the owner and its next words would act with the
//! owner's rights. It is nobody's then.

use std::io;
use std::path::{Path, PathBuf};

use crate::Node;

/// What the node knows of a session the World did not start.
pub(crate) struct Arrival {
    /// The login it runs with, as its own environment names it: none for
    /// the agent's default one; `Unknown` when nothing says.
    pub login: Login,
    /// Whether the user's own program runs it rather than a host that also
    /// runs the sessions of the accounts a login assists.
    pub own: bool,
}

pub(crate) enum Login {
    Named(Option<PathBuf>),
    Unknown,
}

/// The login of `agent` to record for a session the World did not start,
/// written as the node writes logins (none for the default one), when it is
/// the owner's own; why it is no one's otherwise.
pub(crate) fn owners_own(node: &Node, agent: &str, arrival: &Arrival) -> io::Result<Option<PathBuf>> {
    let default = default_login(node, agent)?;
    // As `crate::homes::assistance` writes a login, with the default found once.
    let written =
        |home: Option<&Path>| home.filter(|home| !crate::homes::same_dir(home, &default)).map(Path::to_path_buf);
    let home = match &arrival.login {
        Login::Named(home) => Some(written(home.as_deref())),
        Login::Unknown => None,
    };
    if arrival.own {
        return Ok(home.flatten());
    }
    let shared = shared_logins(node, agent, &written);
    match home {
        Some(home) if shared.iter().any(|other| same_login(other.as_deref(), home.as_deref())) => {
            Err(io::Error::other(format!(
                "the {agent} session runs with {}, a login that works for other accounts here, \
                 and the World did not start it, so it is no one's",
                home.as_deref().map_or_else(|| "the default login".to_string(), |home| home.display().to_string())
            )))
        }
        Some(home) => Ok(home),
        None if !shared.is_empty() => Err(io::Error::other(format!(
            "nothing says which login the {agent} session runs with, and logins here work for other accounts; \
             the World did not start it, so it is no one's"
        ))),
        None => Ok(None),
    }
}

/// Where `agent` keeps its default login on this machine.
fn default_login(node: &Node, agent: &str) -> io::Result<PathBuf> {
    if agent == "codex" {
        return Ok(node.codex.default().config_home().to_path_buf());
    }
    let env = crate::claude::environment().map_err(|failure| io::Error::other(failure.message))?;
    Ok(crate::homes::claude_default(&env, &fastctx_platform::user_home()?))
}

/// This machine's logins of `agent` that serve other accounts: those its
/// networks say assist someone, and those a conversation working for
/// another account runs with, whose records they keep even once the
/// assistance has ended.
fn shared_logins(
    node: &Node,
    agent: &str,
    written: &impl Fn(Option<&Path>) -> Option<PathBuf>,
) -> Vec<Option<PathBuf>> {
    let mut shared = Vec::new();
    for link in node.links() {
        let view = link.view();
        for assist in &view.assists {
            if assist.agent.as_str() == agent && view.is_me(&assist.machine) {
                shared.push(written(assist.home.as_deref()));
            }
        }
    }
    for record in node.residents.everyone() {
        let about = &record.about;
        if about.host.agent.as_str() == agent && crate::assists::assisting(node, &about.acting) {
            shared.push(written(about.home.as_deref()));
        }
    }
    shared
}

/// Whether two logins, as the node writes them, are one.
fn same_login(a: Option<&Path>, b: Option<&Path>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => crate::homes::same_dir(a, b),
        _ => false,
    }
}
