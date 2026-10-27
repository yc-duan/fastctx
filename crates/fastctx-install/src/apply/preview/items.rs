//! The items of a configuration file that differ between what it is and
//! what it would be: each setting by its key path, and the World's section
//! of a guidance file.
//!
//! The files can hold the user's secrets (an MCP server's environment, a
//! token in Claude Code's `env`), so a value is shown only for a key the
//! World itself writes; any other key that would change is named, with its
//! values withheld. The World's edits are ranged, so in practice only its
//! own keys change.

use fastctx_types::logins::{FileHow, ItemChange};
use serde_json::Value;

use crate::guidance::{BEGIN, END, OLD_BEGIN, OLD_END};

/// Whether a key path is one the World writes, whose values may be shown.
pub(super) type Shown = fn(&[String]) -> bool;

/// The World's keys in Codex's `config.toml`.
pub(super) fn codex_keys(path: &[String]) -> bool {
    let path: Vec<&str> = path.iter().map(String::as_str).collect();
    match path.as_slice() {
        ["tool_output_token_limit"]
        | ["features", "shell_tool" | "view_image" | "goals"]
        | ["tools", "update_plan", "enabled"] => true,
        ["mcp_servers", "fastctx", rest @ ..] => matches!(
            rest,
            ["command" | "args" | "env_vars" | "startup_timeout_sec" | "tool_timeout_sec" | "omit_tools_from"]
                | ["env", "FASTCTX_LOGIN_IDENTITY"]
                | ["tools", _, "output_token_limit"]
        ),
        _ => false,
    }
}

/// The World's keys in Claude Code's `settings.json`, with those Claude
/// Code's own plugin commands write for the World's plugin.
pub(super) fn claude_keys(path: &[String]) -> bool {
    let path: Vec<&str> = path.iter().map(String::as_str).collect();
    matches!(
        path.as_slice(),
        ["env", "MAX_MCP_OUTPUT_TOKENS" | "CLAUDE_CODE_MAX_MCP_DESCRIPTION_LENGTH"]
            | ["permissions", "deny"]
            | ["skipDangerousModePermissionPrompt"]
            | ["enabledPlugins", "fastctx-world@fastctx"]
            | ["extraKnownMarketplaces", "fastctx", ..]
    )
}

pub(super) fn toml(before: Option<&[u8]>, after: Option<&[u8]>, shown: Shown) -> Result<Vec<ItemChange>, String> {
    let parse = |bytes: Option<&[u8]>| -> Result<Value, String> {
        let Some(bytes) = bytes else { return Ok(Value::Null) };
        let text =
            std::str::from_utf8(bytes).map_err(|error| format!("Codex's configuration is not UTF-8: {error}"))?;
        let table: toml::Table = text.parse().map_err(|error: toml::de::Error| error.message().to_string())?;
        serde_json::to_value(table).map_err(|error| error.to_string())
    };
    Ok(items(&parse(before)?, &parse(after)?, shown))
}

pub(super) fn json(before: Option<&[u8]>, after: Option<&[u8]>, shown: Shown) -> Result<Vec<ItemChange>, String> {
    let parse = |bytes: Option<&[u8]>| -> Result<Value, String> {
        let Some(bytes) = bytes else { return Ok(Value::Null) };
        let text = std::str::from_utf8(bytes).map_err(|error| format!("Claude settings are not UTF-8: {error}"))?;
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let options =
            jsonc_parser::ParseOptions { allow_comments: true, allow_trailing_commas: true, ..Default::default() };
        jsonc_parser::parse_to_serde_value::<Option<Value>>(text, &options)
            .map(Option::unwrap_or_default)
            .map_err(|error| format!("Invalid Claude settings JSONC: {error}"))
    };
    Ok(items(&parse(before)?, &parse(after)?, shown))
}

/// The World's section of a guidance file, before and after; it is the
/// World's own text.
pub(super) fn guidance(before: Option<&[u8]>, after: Option<&[u8]>) -> Vec<ItemChange> {
    let (before, after) = (section(before), section(after));
    if before == after {
        return Vec::new();
    }
    let how = how(before.is_some(), after.is_some());
    vec![ItemChange { item: "fastctx".into(), how, before, after, withheld: false }]
}

fn section(bytes: Option<&[u8]>) -> Option<String> {
    let bytes = bytes?;
    [(BEGIN, END), (OLD_BEGIN, OLD_END)].into_iter().find_map(|(begin, end)| {
        let start = find(bytes, begin)?;
        let stop = find(&bytes[start..], end)? + start + end.len();
        Some(String::from_utf8_lossy(&bytes[start..stop]).into_owned())
    })
}

fn find(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes.windows(needle.len()).position(|window| window == needle)
}

fn items(before: &Value, after: &Value, shown: Shown) -> Vec<ItemChange> {
    let mut found = Vec::new();
    walk(
        &mut Vec::new(),
        Some(before).filter(|v| !v.is_null()),
        Some(after).filter(|v| !v.is_null()),
        shown,
        &mut found,
    );
    found
}

fn walk(
    path: &mut Vec<String>,
    before: Option<&Value>,
    after: Option<&Value>,
    shown: Shown,
    found: &mut Vec<ItemChange>,
) {
    if before == after {
        return;
    }
    let tables = (before.and_then(Value::as_object), after.and_then(Value::as_object));
    let descend = match tables {
        (Some(_), Some(_)) => true,
        (Some(_), None) => after.is_none(),
        (None, Some(_)) => before.is_none(),
        (None, None) => false,
    };
    if descend {
        let mut keys: Vec<&String> = tables.1.into_iter().flat_map(|table| table.keys()).collect();
        for key in tables.0.into_iter().flat_map(|table| table.keys()) {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        for key in keys {
            path.push(key.clone());
            walk(
                path,
                tables.0.and_then(|table| table.get(key)),
                tables.1.and_then(|table| table.get(key)),
                shown,
                found,
            );
            path.pop();
        }
        return;
    }
    let show = shown(path);
    let text = |value: Option<&Value>| value.filter(|_| show).map(|value| value.to_string());
    let how = how(before.is_some(), after.is_some());
    found.push(ItemChange { item: key(path), how, before: text(before), after: text(after), withheld: !show });
}

pub(super) fn how(before: bool, after: bool) -> FileHow {
    match (before, after) {
        (false, _) => FileHow::Created,
        (true, false) => FileHow::Removed,
        (true, true) => FileHow::Changed,
    }
}

/// A key path as people write it: `features.shell_tool`, with a key that
/// holds a dot or a space quoted.
fn key(path: &[String]) -> String {
    path.iter()
        .map(|part| {
            if part.contains(['.', ' ', '"']) || part.is_empty() {
                serde_json::to_string(part).expect("a key serializes")
            } else {
                part.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}
