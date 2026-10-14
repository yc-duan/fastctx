//! What in a directory shows it is one of an agent's logins, by the names
//! of its entries alone; nothing in it is opened.
//!
//! Two rules. In the usual places (directly in the user's home, named like
//! the agent's own directory) any one of the agent's names is enough. Found
//! anywhere else, a directory needs a harder trace: what holds a login, or
//! a configuration together with what using it leaves behind. A
//! repository's project settings (`.claude/settings.json`,
//! `.codex/config.toml`) are the agents' project layer, not a login, and
//! meet neither rule (`world-agent-logins.md`).

/// An entry of a directory: its name, and whether it is a directory.
pub type Entry = (String, bool);

/// The agents that keep logins, each with the start of its directory's
/// usual name.
pub const AGENTS: [(&str, &str); 2] = [("codex", ".codex"), ("claude", ".claude")];

fn has(entries: &[Entry], name: &str, directory: bool) -> bool {
    entries.iter().any(|(entry, is_dir)| *is_dir == directory && entry == name)
}

fn names(entries: &[Entry], wanted: &[(&str, bool)]) -> Vec<String> {
    wanted
        .iter()
        .filter(|(name, directory)| has(entries, name, *directory))
        .map(|(name, _)| (*name).to_string())
        .collect()
}

/// Whether a directory named `name` directly in the user's home is a
/// login of `agent`: named like its own and holding any of its things.
pub fn usual(agent: &str, name: &str, entries: &[Entry]) -> bool {
    let Some((_, prefix)) = AGENTS.iter().find(|(known, _)| *known == agent) else { return false };
    if !name.starts_with(prefix) {
        return false;
    }
    match agent {
        "codex" => {
            has(entries, "config.toml", false) || has(entries, "auth.json", false) || has(entries, "sessions", true)
        }
        _ => {
            has(entries, "settings.json", false)
                || has(entries, ".credentials.json", false)
                || has(entries, "projects", true)
                || has(entries, ".claude.json", false)
                || has(entries, "daemon.json", false)
        }
    }
}

/// The traces that make a directory found anywhere look like a login of
/// `agent`; none when it does not.
pub fn elsewhere(agent: &str, entries: &[Entry]) -> Option<Vec<String>> {
    match agent {
        "codex" => {
            let used = names(
                entries,
                &[
                    ("sessions", true),
                    ("archived_sessions", true),
                    ("history.jsonl", false),
                    ("session_index.jsonl", false),
                ],
            );
            let state: Vec<String> = entries
                .iter()
                .filter(|(name, directory)| !directory && name.starts_with("state_") && name.ends_with(".sqlite"))
                .map(|(name, _)| name.clone())
                .collect();
            let login = has(entries, "auth.json", false);
            let config = has(entries, "config.toml", false);
            if !login && !(config && (!used.is_empty() || !state.is_empty())) {
                return None;
            }
            let mut traces = names(entries, &[("auth.json", false), ("config.toml", false)]);
            traces.extend(used);
            traces.extend(state);
            Some(traces)
        }
        "claude" => {
            let used = names(
                entries,
                &[
                    ("debug", true),
                    ("statsig", true),
                    ("shell-snapshots", true),
                    ("history.jsonl", false),
                    ("stats-cache.json", false),
                ],
            );
            let login = has(entries, ".credentials.json", false);
            let settings = has(entries, ".claude.json", false) || has(entries, "settings.json", false);
            let kept = settings && has(entries, "projects", true) && !used.is_empty();
            if !(login || kept) {
                return None;
            }
            let mut traces = names(
                entries,
                &[(".credentials.json", false), (".claude.json", false), ("settings.json", false), ("projects", true)],
            );
            traces.extend(used);
            Some(traces)
        }
        _ => None,
    }
}
