//! Read other clients' account-level MCP definitions without starting them.

use crate::Node;
use fastctx_types::mcp::{Found, FoundServer, Source};
use fastctx_types::profile::{Server, parse_servers};
use std::path::PathBuf;

pub(super) fn candidates(node: &Node) -> Result<Vec<(Found, Server)>, String> {
    let home = fastctx_platform::user_home().map_err(|e| e.to_string())?;
    let mut files = vec![(Source::Cursor, home.join(".cursor/mcp.json"))];
    let env = fastctx_platform::user_environment().map_err(|e| e.to_string())?;
    if cfg!(windows) {
        if let Some((_, appdata)) = env.iter().find(|(key, _)| key.eq_ignore_ascii_case("APPDATA")) {
            files.push((Source::ClaudeDesktop, PathBuf::from(appdata).join("Claude/claude_desktop_config.json")));
        }
    } else if cfg!(target_os = "macos") {
        files.push((Source::ClaudeDesktop, home.join("Library/Application Support/Claude/claude_desktop_config.json")));
    }
    files.push((
        Source::ClaudeCode { home: home.join(".claude").to_string_lossy().into_owned() },
        home.join(".claude.json"),
    ));
    for dir in crate::logins::claude_directories(node).map_err(|e| e.message)? {
        files.push((Source::ClaudeCode { home: dir.to_string_lossy().into_owned() }, dir.join(".claude.json")));
    }
    read_files(files)
}

fn read_files(files: Vec<(Source, PathBuf)>) -> Result<Vec<(Found, Server)>, String> {
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (source, path) in files {
        if !seen.insert(path.clone()) {
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("reading {}: {e}", path.display())),
        };
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("reading {}: {e}", path.display()))?;
        let Some(servers) = value.get("mcpServers") else {
            continue;
        };
        let servers = servers.as_object().ok_or("mcpServers is not an object")?;
        for (name, entry) in servers {
            if name.eq_ignore_ascii_case(fastctx_types::profile::RESERVED) {
                continue;
            }
            let wrapped = serde_json::json!({"candidate":entry});
            let (_, server) = parse_servers(&wrapped.to_string())?.pop().expect("one candidate");
            let name = name.clone();
            let shown = match &server {
                Server::Command(_) => FoundServer::Command,
                Server::Remote(remote) => FoundServer::Remote(remote.clone()),
            };
            result.push((Found { source: source.clone(), name, server: shown }, server));
        }
    }
    Ok(result)
}
