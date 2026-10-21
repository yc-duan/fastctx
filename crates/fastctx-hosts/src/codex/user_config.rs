//! A thread's account MCP entries must never merge with login credentials.

use super::{AppServer, ThreadSpec};
use serde_json::json;
use std::collections::BTreeSet;

pub(super) async fn effective(server: &AppServer, spec: &ThreadSpec, assisting: bool) -> Result<ThreadSpec, String> {
    if spec.user_mcp.is_empty() && !assisting {
        return Ok(spec.clone());
    }
    let answer = server.request("config/read", json!({"cwd":spec.cwd,"includeLayers":false})).await?;
    let mut occupied: BTreeSet<String> =
        answer["config"]["mcp_servers"].as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
    if assisting {
        let mut effective = spec.clone();
        for name in &occupied {
            if name != super::SERVER_NAME {
                effective.config["mcp_servers"][name] = json!({"enabled":false});
            }
        }
        super::assisted::configure_thread(&mut effective.config);
        return Ok(effective);
    }
    let inherited = occupied.clone();
    let mut effective = spec.clone();
    let servers = effective.config["mcp_servers"].as_object_mut().ok_or("the thread has no MCP table")?;
    occupied.extend(servers.keys().cloned());
    let mut renamed = Vec::new();
    for name in &spec.user_mcp {
        if !inherited.contains(name) {
            continue;
        }
        let Some(server) = servers.remove(name) else {
            continue;
        };
        let stem = format!("user_{}", &name[..name.len().min(24)]);
        let alias = (0u64..)
            .map(|n| if n == 0 { stem.clone() } else { format!("{stem}_{n}") })
            .find(|n| !occupied.contains(n))
            .expect("an unused name");
        occupied.insert(alias.clone());
        renamed.push(format!("{name} is available as {alias}"));
        servers.insert(alias, server);
    }
    if !renamed.is_empty() {
        effective
            .developer_instructions
            .push_str(&format!("\n\nYour account's MCP servers in this thread: {}.", renamed.join("; ")));
    }
    Ok(effective)
}
