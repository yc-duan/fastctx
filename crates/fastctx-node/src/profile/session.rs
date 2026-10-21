//! The account layer of one conversation, generated without touching a login.

use crate::Node;
use fastctx_types::{
    ResidentId,
    access::Acting,
    ipc::Failure,
    profile::{Entry, Profile},
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;

pub(crate) const INTRO: &str = "The user's own instructions for all work for them; follow them:";
pub(crate) const CHANGED: &str =
    "=== the user's own instructions changed; they now read as follows, and replace those you were given ===";

/// A first use can arrive before the hub's directory notification that
/// grants this machine the account's full profile. Read that acknowledged view.
pub(crate) async fn ready(node: &Node, acting: &Acting) -> Result<(), Failure> {
    let Acting::Accounts { accounts } = acting else {
        return Ok(());
    };
    let [account] = accounts.as_slice() else {
        return Ok(());
    };
    let link = node
        .link_of(&account.hub)
        .ok_or_else(|| Failure::not_run("the conversation's account is no longer connected"))?;
    if !link.view().may_act_for(&account.account, &link.view().me) {
        link.refresh().await.map_err(Failure::new)?;
    }
    if link.view().profiles {
        link.profile_read(&account.account).map_err(Failure::new)?;
    }
    Ok(())
}

pub(crate) fn get(node: &Node, acting: &Acting) -> Profile {
    let Acting::Accounts { accounts } = acting else { return Profile::default() };
    let [account] = accounts.as_slice() else { return Profile::default() };
    node.link_of(&account.hub)
        .and_then(|link| link.profile_read(&account.account).ok())
        .filter(|v| v.kept)
        .map(|v| v.profile)
        .unwrap_or_default()
}
pub(crate) fn prompt(node: &Node, acting: &Acting) -> String {
    get(node, acting).prompt
}
pub(crate) fn hash(prompt: &str) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(prompt.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}
pub(crate) fn text(prompt: &str) -> String {
    if prompt.is_empty() { String::new() } else { format!("{INTRO}\n{prompt}") }
}

fn servers(
    node: &Node,
    id: &ResidentId,
    acting: &Acting,
    off: &BTreeSet<String>,
    codex: bool,
) -> serde_json::Map<String, Value> {
    let profile = get(node, acting);
    let mut servers = serde_json::Map::new();
    for (name, entry) in profile.servers {
        if off.contains(&name) {
            continue;
        }
        let value = match entry {
            Entry::Remote(remote) if codex && !remote.sse => {
                json!({"url":remote.url,"http_headers":remote.headers,"required":false})
            }
            Entry::Remote(remote) if !codex => {
                json!({"type":if remote.sse {"sse"}else{"http"},"url":remote.url,"headers":remote.headers})
            }
            Entry::Command { machine } => {
                let Acting::Accounts { accounts } = acting else { continue };
                let Some(link) = accounts.first().and_then(|account| node.link_of(&account.hub)) else { continue };
                let view = link.view();
                if !view.is_me(&machine) && !view.member(&machine).is_some_and(|m| m.online) {
                    continue;
                }
                json!({"command":node.fastctx,"args":["mcp-relay",name],"env":{"FASTCTX_HOME":node.root,"FASTCTX_RESIDENT":fastctx_types::Address {machine:node.machine(),resident:id.clone()}.tag()}})
            }
            _ => continue,
        };
        servers.insert(name, value);
    }
    servers
}

pub(crate) fn codex(
    node: &Node,
    id: &ResidentId,
    acting: &Acting,
    config: &mut Value,
) -> Result<BTreeSet<String>, Failure> {
    let profile = get(node, acting);
    // Only what an account may set, whatever its hub sent.
    for (key, value) in profile.codex {
        if fastctx_types::profile::CODEX_SETTINGS.contains(&key.as_str()) {
            config[&key] = value;
        }
    }
    let options = crate::session_options::read(node, id)?;
    let added = servers(node, id, acting, &options.mcp_off, true);
    let names = added.keys().cloned().collect();
    for (name, server) in added {
        config["mcp_servers"][name] = server;
    }
    Ok(names)
}

pub(crate) fn claude(node: &Node, id: &ResidentId, creating: bool) -> Result<std::path::PathBuf, Failure> {
    let (about, _) = node.residents.about(id).ok_or_else(|| Failure::new("unknown conversation"))?;
    let directory = node.world.conversation(id);
    let mcp = directory.path().join("claude-mcp.json");
    let profile = get(node, &about.acting);
    let options = crate::session_options::read(node, id)?;
    let mut settings: Value = serde_json::from_str(&crate::claude::settings_json(id)).expect("session settings");
    for (key, value) in profile.claude {
        if key == "env" {
            // The account's variables join the session's own, which the
            // plugin reads and which are never written over; only those an
            // account may set.
            let Value::Object(theirs) = value else { continue };
            let own = settings["env"].as_object_mut().expect("session settings carry an env");
            for (name, value) in theirs {
                if fastctx_types::profile::CLAUDE_ENV.contains(&name.as_str()) {
                    own.entry(name).or_insert(value);
                }
            }
            continue;
        }
        if fastctx_types::profile::CLAUDE_SETTINGS.contains(&key.as_str()) {
            settings[&key] = value;
        }
    }
    let assisting = crate::assists::host::starting(node, id, &about.acting)?;
    if assisting {
        let env = crate::claude::environment()?;
        let user_home = fastctx_platform::user_home().map_err(|error| Failure::new(error.to_string()))?;
        let default = crate::homes::claude_default(&env, &user_home);
        fastctx_hosts::claude::assisted::configure(&mut settings, about.home.as_deref().unwrap_or(&default), &env)
            .map_err(Failure::new)?;
    }
    fastctx_platform::publish(
        &directory.claude_settings(),
        &serde_json::to_vec_pretty(&settings).unwrap(),
        fastctx_platform::Permissions::Private,
    )
    .map_err(|e| Failure::new(e.to_string()))?;
    if creating || mcp.is_file() {
        let servers = if assisting {
            // Strict MCP mode excludes plugin servers too, including the
            // World's. Supply that server explicitly under its usual name.
            json!({"fastctx": {
                "command": node.fastctx,
                "args": ["serve"],
                "env": {
                    "FASTCTX_HOME": node.root,
                    "FASTCTX_AGENT_HOME": about.home.as_deref().map(|home| home.to_string_lossy()).unwrap_or_default(),
                },
            }})
        } else {
            Value::Object(servers(node, id, &about.acting, &options.mcp_off, false))
        };
        let value = json!({"mcpServers": servers});
        fastctx_platform::publish(
            &mcp,
            &serde_json::to_vec_pretty(&value).unwrap(),
            fastctx_platform::Permissions::Private,
        )
        .map_err(|e| Failure::new(e.to_string()))?;
    }
    if let Some(session) = &about.session {
        let directory = fastctx_hosts::claude::exchange::sessions(node.world.path()).join(session);
        hand_to_session(node, id, &directory).map_err(|e| Failure::new(e.to_string()))?;
    }
    Ok(mcp)
}

/// Writes the account's prompt into the session's directory, reserved
/// already: once its records are gathered into the conversation, that path
/// is the room's alias, which is made private as the room is.
pub(crate) fn hand_to_session(node: &Node, id: &ResidentId, session: &std::path::Path) -> std::io::Result<()> {
    let prompt = node.residents.about(id).map(|(about, _)| prompt(node, &about.acting)).unwrap_or_default();
    // An empty block also replaces an earlier prompt after account access is lost.
    fastctx_platform::publish(
        &session.join("user.md"),
        text(&prompt).as_bytes(),
        fastctx_platform::Permissions::Private,
    )
}

pub(crate) async fn refresh(node: &Arc<Node>) {
    for resident in node.residents.everyone() {
        if resident.removed.is_some() || !resident.about.world_started {
            continue;
        }
        let Ok(gate) = node.residents.management(&resident.id) else {
            continue;
        };
        let Ok(_held) = tokio::time::timeout(std::time::Duration::from_millis(1), gate.read()).await else {
            continue;
        };
        if node.residents.active(&resident.id).is_err() {
            continue;
        }
        if resident.about.host.agent.as_str() == "claude" {
            if let Err(e) = claude(node, &resident.id, false) {
                node.log.line(format_args!("updating {}'s user configuration: {e}", resident.id));
            }
        } else if resident.about.host.agent.as_str() == "codex" {
            crate::codex::moved(node, &resident.id).await;
        }
    }
}

pub(crate) fn start(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        let mut views = node.views.subscribe();
        let mut ways = node.ways.subscribe();
        loop {
            refresh(&node).await;
            for link in node.links() {
                let result = if matches!(link.status(), fastctx_net::link::Status::Stopped { .. }) {
                    crate::mcp::store::leave(&node, &link.membership().id)
                } else {
                    crate::mcp::store::sweep(&node, &link.membership().id)
                };
                if let Err(error) = result {
                    node.log.line(format_args!("collecting MCP commands: {error}"));
                }
            }
            tokio::select! {
                _=views.changed()=>{}, _=ways.changed()=>{},
                _=tokio::time::sleep(std::time::Duration::from_secs(60))=>{},
            }
        }
    });
}
