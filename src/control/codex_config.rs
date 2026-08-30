//! Ownership-aware Codex TOML editing with `toml_edit` preserving all unowned content.

use crate::control::settings::{Tier, ToolBudgets};
use crate::server_manifest::EnabledTools;
use std::ops::Range;
use std::str::FromStr;
use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

const FASTCTX_NAMESPACE: &str = "mcp__fastctx";
const LEGACY_FASTREAD_NAMESPACE: &str = "mcp__fastread";
const LEGACY_FASTSHELL_NAMESPACE: &str = "mcp__fastshell";
const LEGACY_FASTEDIT_NAMESPACE: &str = "mcp__fastedit";
const STARTUP_TIMEOUT_SECONDS: i64 = 120;
/// MCP tool timeout written by Apply so 240-second tool waits retain a 60-second return margin.
pub(crate) const TOOL_TIMEOUT_SECONDS: i64 = 300;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct LegacyRemoval {
    fastread: bool,
    fastshell: bool,
    fastedit: bool,
}

/// Expected Codex configuration after Apply.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectedConfig {
    /// Stable absolute command path.
    pub command: String,
    /// Host output tier.
    pub tier: Tier,
    /// Effective host output limit; may be Guarded independently from the selected tier.
    pub host_limit: i64,
    /// Effective global FastCtx budget; may be Guarded independently from the selected tier.
    pub fastctx_budget: usize,
    /// Five long-output tools' relative budgets.
    pub tool_budgets: ToolBudgets,
    /// Exact validated tool set published for this target.
    pub enabled_tools: EnabledTools,
}

/// Conflict on a shared key in an Apply plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenLimitConflict {
    /// Current user setting.
    pub current: i64,
    /// Setting required by the FastCtx tier.
    pub requested: i64,
}

/// Independent ownership evidence for the two Codex configuration values managed by Apply.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CodexConfigOwnership {
    /// Whether the `mcp_servers.fastctx` entry is owned by a FastCtx receipt.
    pub server_entry_owned: bool,
    /// Whether FastCtx inserted the surviving `mcp__fastctx` namespace occurrence.
    pub direct_namespace_inserted: bool,
}

/// Immutable Codex-config edit result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplyEdit {
    /// Complete new file bytes.
    pub bytes: Vec<u8>,
    /// Whether the shared token key originally existed.
    pub previous_token_limit_present: bool,
    /// Previous value of the shared token key.
    pub previous_token_limit: Option<i64>,
    /// A present and differing shared key requires additional confirmation.
    pub conflict: Option<TokenLimitConflict>,
    /// Ownership to persist in the successful Apply receipt.
    pub ownership: CodexConfigOwnership,
}

/// Parses Codex TOML and produces the post-Apply bytes.
pub fn apply(
    original: &[u8],
    expected: &ExpectedConfig,
    previous_ownership: CodexConfigOwnership,
) -> Result<ApplyEdit, String> {
    let (migrated, legacy) = strip_owned_legacy_servers(original, expected)?;
    let mut document = parse(&migrated)?;
    let requested_limit = expected.host_limit;
    let existing_limit = document.get("tool_output_token_limit");
    let previous_token_limit_present = existing_limit.is_some();
    let previous_token_limit = existing_limit
        .map(|item| {
            item.as_integer().ok_or_else(|| {
                "Codex config key tool_output_token_limit is not an integer. Repair it manually and retry."
                    .to_string()
            })
        })
        .transpose()?;
    let conflict = previous_token_limit
        .filter(|current| *current != requested_limit)
        .map(|current| TokenLimitConflict {
            current,
            requested: requested_limit,
        });

    let mcp_servers = ensure_table(&mut document, "mcp_servers")?;
    let mut fastctx_table = build_fastctx_table(expected);
    if let Some(existing) = mcp_servers.get("fastctx") {
        if !previous_ownership.server_entry_owned {
            return Err(
                "Codex config already contains mcp_servers.fastctx, but the current FastCtx receipt does not own it. Rename or remove that entry manually, or restore its matching FastCtx receipt, then retry Apply."
                    .to_string(),
            );
        }
        validate_owned_fastctx_entry(existing)?;
        let existing = existing
            .as_table()
            .expect("validated receipt-owned FastCtx entry must be a table");
        *fastctx_table.decor_mut() = existing.decor().clone();
    }
    mcp_servers.insert("fastctx", Item::Table(fastctx_table));

    let direct_namespace_inserted;
    let features = ensure_table(&mut document, "features")?;
    let code_mode = ensure_child_table(features, "code_mode", "features.code_mode")?;
    match code_mode.get_mut("direct_only_tool_namespaces") {
        Some(item) => {
            let array = item.as_array_mut().ok_or_else(|| {
                "Codex config key features.code_mode.direct_only_tool_namespaces is not an array. Repair it manually and retry."
                    .to_string()
            })?;
            if legacy.fastread {
                reconcile_namespace(array, LEGACY_FASTREAD_NAMESPACE, false);
            }
            if legacy.fastshell {
                reconcile_namespace(array, LEGACY_FASTSHELL_NAMESPACE, false);
            }
            if legacy.fastedit {
                reconcile_namespace(array, LEGACY_FASTEDIT_NAMESPACE, false);
            }
            let count = namespace_count(array, FASTCTX_NAMESPACE);
            if count > 1 {
                return Err(
                    "Codex config contains multiple mcp__fastctx direct-only namespaces. Remove the duplicates manually and retry Apply; FastCtx cannot infer which occurrence is user-owned."
                        .to_string(),
                );
            }
            if count == 0 {
                push_preserving_array_trailing(array, FASTCTX_NAMESPACE);
                direct_namespace_inserted = true;
            } else {
                direct_namespace_inserted = previous_ownership.direct_namespace_inserted;
            }
        }
        None => {
            let mut array = Array::new();
            array.push(FASTCTX_NAMESPACE);
            code_mode.insert(
                "direct_only_tool_namespaces",
                Item::Value(Value::Array(array)),
            );
            direct_namespace_inserted = true;
        }
    }
    set_integer(&mut document, "tool_output_token_limit", requested_limit)?;

    Ok(ApplyEdit {
        bytes: document.to_string().into_bytes(),
        previous_token_limit_present,
        previous_token_limit,
        conflict,
        ownership: CodexConfigOwnership {
            server_entry_owned: true,
            direct_namespace_inserted,
        },
    })
}

/// Removes FastCtx-owned configuration in reverse; the shared token key is restored only when explicitly allowed.
pub fn unapply(
    original: &[u8],
    ownership: CodexConfigOwnership,
    restore_token_limit: bool,
    previous_token_limit_present: bool,
    previous_token_limit: Option<i64>,
) -> Result<Vec<u8>, String> {
    let mut document = parse(original)?;
    if ownership.server_entry_owned && document.get("mcp_servers").is_some() {
        let (removed, emptied) = {
            let mcp_servers = document
                .get_mut("mcp_servers")
                .and_then(Item::as_table_mut)
                .ok_or_else(|| {
                    "Codex config key mcp_servers is not a table. Repair it manually and retry."
                        .to_string()
                })?;
            if let Some(existing) = mcp_servers.get("fastctx") {
                validate_owned_fastctx_entry(existing)?;
            }
            let removed = mcp_servers.remove("fastctx").is_some();
            (removed, mcp_servers.is_empty())
        };
        // Remove an mcp_servers table created solely by Apply so no empty shell remains;
        // this also keeps drift-free reverse removal byte-exact with Apply (2026-07-12).
        if removed && emptied {
            document.remove("mcp_servers");
        }
    }

    if ownership.direct_namespace_inserted && document.get("features").is_some() {
        let (removed, emptied) = {
            let features = document
                .get_mut("features")
                .and_then(Item::as_table_mut)
                .ok_or_else(|| {
                    "Codex config key features is not a table. Repair it manually and retry."
                        .to_string()
                })?;
            let mut remove_code_mode = false;
            let mut removed = false;
            if features.get("code_mode").is_some() {
                let code_mode = features
                    .get_mut("code_mode")
                    .and_then(Item::as_table_mut)
                    .ok_or_else(|| {
                        "Codex config key features.code_mode is not a table. Repair it manually and retry."
                            .to_string()
                    })?;
                if code_mode.get("direct_only_tool_namespaces").is_some() {
                    let array = code_mode
                        .get_mut("direct_only_tool_namespaces")
                        .and_then(Item::as_array_mut)
                        .ok_or_else(|| {
                            "Codex config key features.code_mode.direct_only_tool_namespaces is not an array. Repair it manually and retry."
                                .to_string()
                        })?;
                    let matching = namespace_indices(array, FASTCTX_NAMESPACE);
                    if matching.len() > 1 {
                        return Err(
                            "Codex config contains multiple mcp__fastctx direct-only namespaces. Disconnect stopped because it cannot safely identify the receipt-owned occurrence. Remove duplicates manually and retry."
                                .to_string(),
                        );
                    }
                    if let Some(index) = matching.first().copied() {
                        remove_array_index_preserving_trailing(array, index);
                        removed = true;
                    }
                    if removed && array.is_empty() {
                        code_mode.remove("direct_only_tool_namespaces");
                    }
                }
                remove_code_mode = removed && code_mode.is_empty();
            }
            if remove_code_mode {
                features.remove("code_mode");
            }
            (removed, features.is_empty())
        };
        // Likewise remove a features table created solely by Apply after code_mode is removed.
        if removed && emptied {
            document.remove("features");
        }
    }

    if restore_token_limit {
        if previous_token_limit_present {
            let previous = previous_token_limit.ok_or_else(|| {
                "The Apply receipt says tool_output_token_limit existed but does not contain its previous value. Re-apply before restoring it."
                    .to_string()
            })?;
            set_integer(&mut document, "tool_output_token_limit", previous)?;
        } else {
            document.remove("tool_output_token_limit");
        }
    }
    Ok(document.to_string().into_bytes())
}

/// Reads the current integer tool_output_token_limit, returning None when absent, non-integer, or unparseable.
/// Unapply uses this for ownership: restore the shared key only while it still equals the value written by Apply.
pub fn current_token_limit(original: &[u8]) -> Option<i64> {
    parse(original)
        .ok()?
        .get("tool_output_token_limit")
        .and_then(Item::as_integer)
}

/// Returns whether a managed server table exists in a valid Codex config.
pub fn has_server(original: &[u8], name: &str) -> bool {
    parse(original)
        .ok()
        .and_then(|document| {
            document
                .get("mcp_servers")
                .and_then(Item::as_table_like)
                .and_then(|table| table.get(name))
                .map(|_| ())
        })
        .is_some()
}

/// Returns whether the direct-only namespace array contains an entry.
pub fn has_namespace(original: &[u8], namespace: &str) -> bool {
    parse(original)
        .ok()
        .and_then(|document| {
            document
                .get("features")
                .and_then(Item::as_table_like)
                .and_then(|table| table.get("code_mode"))
                .and_then(Item::as_table_like)
                .and_then(|table| table.get("direct_only_tool_namespaces"))
                .and_then(Item::as_array)
                .map(|array| array.iter().any(|entry| entry.as_str() == Some(namespace)))
        })
        .unwrap_or(false)
}

/// Checks each Codex configuration item against the Apply receipt.
pub fn drift(original: &[u8], expected: &ExpectedConfig) -> Result<Vec<String>, String> {
    drift_with_limits(
        original,
        expected,
        expected.host_limit,
        expected.fastctx_budget,
        Some(TOOL_TIMEOUT_SECONDS),
    )
}

/// Checks managed configuration against the exact numeric values recorded by an Apply receipt.
/// Older receipts omit `tool_timeout_sec`; that missing ownership evidence must not create false drift.
pub fn drift_applied(
    original: &[u8],
    expected: &ExpectedConfig,
    host_limit: i64,
    fastctx_budget: usize,
    tool_timeout_sec: Option<i64>,
) -> Result<Vec<String>, String> {
    drift_with_limits(
        original,
        expected,
        host_limit,
        fastctx_budget,
        tool_timeout_sec,
    )
}

fn drift_with_limits(
    original: &[u8],
    expected: &ExpectedConfig,
    host_limit: i64,
    fastctx_budget: usize,
    tool_timeout_sec: Option<i64>,
) -> Result<Vec<String>, String> {
    let document = parse(original)?;
    let mut drift = Vec::new();
    let fastctx = document
        .get("mcp_servers")
        .and_then(Item::as_table_like)
        .and_then(|table| table.get("fastctx"))
        .and_then(Item::as_table_like);
    match fastctx {
        Some(table) => {
            for (key, _) in table.iter() {
                if ![
                    "command",
                    "args",
                    "startup_timeout_sec",
                    "tool_timeout_sec",
                    "env",
                ]
                .contains(&key)
                {
                    drift.push(format!("mcp_servers.fastctx.{key}"));
                }
            }
            if table.get("command").and_then(Item::as_str) != Some(expected.command.as_str()) {
                drift.push("mcp_servers.fastctx.command".to_string());
            }
            if table.get("startup_timeout_sec").and_then(Item::as_integer)
                != Some(STARTUP_TIMEOUT_SECONDS)
            {
                drift.push("mcp_servers.fastctx.startup_timeout_sec".to_string());
            }
            if let Some(tool_timeout_sec) = tool_timeout_sec
                && table.get("tool_timeout_sec").and_then(Item::as_integer)
                    != Some(tool_timeout_sec)
            {
                drift.push("mcp_servers.fastctx.tool_timeout_sec".to_string());
            }
            let env = table
                .get("env")
                .and_then(Item::as_table_like)
                .ok_or_else(|| "mcp_servers.fastctx.env is missing or not a table".to_string());
            match env {
                Ok(env) => {
                    for (key, _) in env.iter() {
                        if ![
                            "FASTCTX_TOKEN_BUDGET",
                            "FASTCTX_READ_TOKEN_BUDGET",
                            "FASTCTX_GREP_TOKEN_BUDGET",
                            "FASTCTX_GLOB_TOKEN_BUDGET",
                            "FASTCTX_RUN_TOKEN_BUDGET",
                            "FASTCTX_JOB_OUTPUT_TOKEN_BUDGET",
                        ]
                        .contains(&key)
                        {
                            drift.push(format!("mcp_servers.fastctx.env.{key}"));
                        }
                    }
                    check_env(env, expected, fastctx_budget, &mut drift)
                }
                Err(_) => drift.push("mcp_servers.fastctx.env".to_string()),
            }
            let actual_args = table.get("args").and_then(Item::as_array);
            let expected_args = server_args(expected);
            let args_match = actual_args.is_some_and(|args| {
                args.len() == expected_args.len()
                    && args
                        .iter()
                        .zip(expected_args.iter())
                        .all(|(actual, expected)| actual.as_str() == Some(expected.as_str()))
            });
            if !args_match {
                drift.push("mcp_servers.fastctx.args".to_string());
            }
        }
        None => drift.push("mcp_servers.fastctx".to_string()),
    }
    let namespaces = document
        .get("features")
        .and_then(Item::as_table_like)
        .and_then(|table| table.get("code_mode"))
        .and_then(Item::as_table_like)
        .and_then(|table| table.get("direct_only_tool_namespaces"))
        .and_then(Item::as_array);
    let count = namespaces
        .map(|array| {
            array
                .iter()
                .filter(|entry| entry.as_str() == Some(FASTCTX_NAMESPACE))
                .count()
        })
        .unwrap_or(0);
    if count != 1 {
        drift.push(format!(
            "features.code_mode.direct_only_tool_namespaces[{FASTCTX_NAMESPACE}]"
        ));
    }
    if document
        .get("tool_output_token_limit")
        .and_then(Item::as_integer)
        != Some(host_limit)
    {
        drift.push("tool_output_token_limit".to_string());
    }
    Ok(drift)
}

fn parse(original: &[u8]) -> Result<DocumentMut, String> {
    let source = std::str::from_utf8(original).map_err(|error| {
        format!("Codex config is not valid UTF-8 ({error}). Repair the file manually and retry.")
    })?;
    DocumentMut::from_str(source).map_err(|error| {
        format!("Cannot parse Codex config.toml: {error}. Repair it manually and retry.")
    })
}

fn set_integer(document: &mut DocumentMut, key: &str, integer: i64) -> Result<(), String> {
    match document.get_mut(key) {
        Some(item) => {
            let existing = item.as_value_mut().ok_or_else(|| {
                format!("Codex config key {key} is not an integer. Repair it manually and retry.")
            })?;
            if existing.as_integer().is_none() {
                return Err(format!(
                    "Codex config key {key} is not an integer. Repair it manually and retry."
                ));
            }
            let decor = existing.decor().clone();
            let mut replacement = Value::from(integer);
            *replacement.decor_mut() = decor;
            *existing = replacement;
        }
        None => document[key] = value(integer),
    }
    Ok(())
}

fn push_preserving_array_trailing(array: &mut Array, entry: &str) {
    let trailing = array
        .len()
        .checked_sub(1)
        .and_then(|index| array.get_mut(index))
        .and_then(|value| value.decor().suffix().cloned());
    if let Some(last) = array
        .len()
        .checked_sub(1)
        .and_then(|index| array.get_mut(index))
    {
        last.decor_mut().set_suffix("");
    }
    let mut value = Value::from(entry);
    value
        .decor_mut()
        .set_prefix(if array.is_empty() { "" } else { " " });
    if let Some(trailing) = trailing {
        value.decor_mut().set_suffix(trailing);
    }
    array.push_formatted(value);
}

fn reconcile_namespace(array: &mut Array, namespace: &str, enabled: bool) {
    let matching = namespace_indices(array, namespace);
    let keep = enabled.then(|| matching.first().copied()).flatten();
    for index in matching.into_iter().rev() {
        if Some(index) != keep {
            remove_array_index_preserving_trailing(array, index);
        }
    }
    if enabled && keep.is_none() {
        push_preserving_array_trailing(array, namespace);
    }
}

fn namespace_indices(array: &Array, namespace: &str) -> Vec<usize> {
    array
        .iter()
        .enumerate()
        .filter_map(|(index, value)| (value.as_str() == Some(namespace)).then_some(index))
        .collect()
}

fn namespace_count(array: &Array, namespace: &str) -> usize {
    array
        .iter()
        .filter(|value| value.as_str() == Some(namespace))
        .count()
}

fn remove_array_index_preserving_trailing(array: &mut Array, index: usize) {
    let suffix = (index + 1 == array.len())
        .then(|| {
            array
                .get(index)
                .and_then(|value| value.decor().suffix().cloned())
        })
        .flatten();
    array.remove(index);
    if let (Some(previous), Some(suffix)) = (
        index.checked_sub(1).and_then(|index| array.get_mut(index)),
        suffix,
    ) {
        previous.decor_mut().set_suffix(suffix);
    }
}

fn ensure_table<'a>(document: &'a mut DocumentMut, key: &str) -> Result<&'a mut Table, String> {
    if document.get(key).is_none() {
        document[key] = Item::Table(Table::new());
    }
    document
        .get_mut(key)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| {
            format!("Codex config key {key} is not a table. Repair it manually and retry.")
        })
}

fn ensure_child_table<'a>(
    parent: &'a mut Table,
    key: &str,
    display: &str,
) -> Result<&'a mut Table, String> {
    if parent.get(key).is_none() {
        parent.insert(key, Item::Table(Table::new()));
    }
    parent
        .get_mut(key)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| {
            format!("Codex config key {display} is not a table. Repair it manually and retry.")
        })
}

fn build_fastctx_table(expected: &ExpectedConfig) -> Table {
    let global = expected.fastctx_budget;
    let mut table = Table::new();
    table.insert("command", value(expected.command.clone()));
    let mut args = Array::new();
    for argument in server_args(expected) {
        args.push(argument);
    }
    table.insert("args", Item::Value(Value::Array(args)));
    table.insert("startup_timeout_sec", value(STARTUP_TIMEOUT_SECONDS));
    table.insert("tool_timeout_sec", value(TOOL_TIMEOUT_SECONDS));
    let mut env = Table::new();
    env.insert("FASTCTX_TOKEN_BUDGET", value(global.to_string()));
    for (tool, key, budget) in expected_tool_budgets(expected) {
        if expected.enabled_tools.contains(tool) {
            insert_tool_budget(&mut env, key, budget);
        }
    }
    table.insert("env", Item::Table(env));
    table
}

fn validate_owned_fastctx_entry(item: &Item) -> Result<(), String> {
    let table = item.as_table().ok_or_else(|| {
        "The receipt-owned Codex key mcp_servers.fastctx is no longer a table. Repair or remove the drifted entry manually and retry Apply."
            .to_string()
    })?;
    let allowed = [
        "command",
        "args",
        "startup_timeout_sec",
        "tool_timeout_sec",
        "env",
    ];
    let unknown = table
        .iter()
        .filter_map(|(key, _)| (!allowed.contains(&key)).then_some(key.to_string()))
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(format!(
            "The receipt-owned Codex entry mcp_servers.fastctx contains unknown keys ({}). Move or remove those user values before Apply; FastCtx will not overwrite them.",
            unknown.join(", ")
        ));
    }
    if table.get("env").is_some() && table.get("env").and_then(Item::as_table_like).is_none() {
        return Err(
            "The receipt-owned Codex key mcp_servers.fastctx.env is no longer a table. Repair or remove the drifted value manually and retry Apply."
                .to_string(),
        );
    }
    if let Some(env) = table.get("env").and_then(Item::as_table_like) {
        let allowed_env = [
            "FASTCTX_TOKEN_BUDGET",
            "FASTCTX_READ_TOKEN_BUDGET",
            "FASTCTX_GREP_TOKEN_BUDGET",
            "FASTCTX_GLOB_TOKEN_BUDGET",
            "FASTCTX_RUN_TOKEN_BUDGET",
            "FASTCTX_JOB_OUTPUT_TOKEN_BUDGET",
        ];
        let unknown = env
            .iter()
            .filter_map(|(key, _)| (!allowed_env.contains(&key)).then_some(key.to_string()))
            .collect::<Vec<_>>();
        if !unknown.is_empty() {
            return Err(format!(
                "The receipt-owned Codex entry mcp_servers.fastctx.env contains unknown keys ({}). Move or remove those user values before Apply; FastCtx will not overwrite them.",
                unknown.join(", ")
            ));
        }
    }
    Ok(())
}

fn owned_legacy_servers(mcp_servers: &Table, expected: &ExpectedConfig) -> LegacyRemoval {
    let fastread = mcp_servers
        .get("fastread")
        .is_some_and(legacy_fastread_table_is_owned);
    let fastshell = mcp_servers.get("fastshell").is_some_and(|item| {
        legacy_optional_table_is_owned(item, expected, "shell-serve", "FASTSHELL_TOKEN_BUDGET")
    });
    let fastedit = mcp_servers.get("fastedit").is_some_and(|item| {
        legacy_optional_table_is_owned(item, expected, "edit-serve", "FASTEDIT_TOKEN_BUDGET")
    });
    LegacyRemoval {
        fastread,
        fastshell,
        fastedit,
    }
}

fn strip_owned_legacy_servers(
    original: &[u8],
    expected: &ExpectedConfig,
) -> Result<(Vec<u8>, LegacyRemoval), String> {
    let source = std::str::from_utf8(original).map_err(|error| {
        format!("Codex config is not valid UTF-8 ({error}). Repair the file manually and retry.")
    })?;
    let document = toml_edit::ImDocument::parse(source).map_err(|error| {
        format!("Cannot parse Codex config.toml: {error}. Repair it manually and retry.")
    })?;
    let Some(mcp_servers) = document.get("mcp_servers").and_then(Item::as_table) else {
        return Ok((original.to_vec(), LegacyRemoval::default()));
    };
    let legacy = owned_legacy_servers(mcp_servers, expected);
    let mut spans = Vec::new();
    for (name, owned) in [
        ("fastread", legacy.fastread),
        ("fastshell", legacy.fastshell),
        ("fastedit", legacy.fastedit),
    ] {
        if owned {
            let item = mcp_servers
                .get(name)
                .expect("an owned legacy table must still exist");
            collect_explicit_table_spans(item, name, &mut spans)?;
        }
    }
    if spans.is_empty() {
        return Ok((original.to_vec(), legacy));
    }
    spans.sort_by_key(|span| span.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(spans.len());
    for span in spans {
        if let Some(previous) = merged.last_mut()
            && span.start <= previous.end
        {
            previous.end = previous.end.max(span.end);
        } else {
            merged.push(span);
        }
    }
    let mut output = Vec::with_capacity(original.len());
    let mut cursor = 0;
    for span in merged {
        output.extend_from_slice(&original[cursor..span.start]);
        cursor = span.end;
    }
    output.extend_from_slice(&original[cursor..]);
    Ok((output, legacy))
}

fn collect_explicit_table_spans(
    item: &Item,
    name: &str,
    spans: &mut Vec<Range<usize>>,
) -> Result<(), String> {
    let table = item.as_table().ok_or_else(|| {
        format!("Cannot safely migrate mcp_servers.{name}: the owned entry is not a table.")
    })?;
    if !table.is_implicit() {
        spans.push(table.span().ok_or_else(|| {
            format!(
                "Cannot safely migrate mcp_servers.{name}: its source range is unavailable. Repair or remove the legacy table manually and retry."
            )
        })?);
    }
    for (_, child) in table.iter() {
        if child.is_table() {
            collect_explicit_table_spans(child, name, spans)?;
        }
    }
    Ok(())
}

fn legacy_optional_table_is_owned(
    item: &Item,
    expected: &ExpectedConfig,
    argument: &str,
    budget_key: &str,
) -> bool {
    let Some(table) = item.as_table_like() else {
        return false;
    };
    if !has_exact_keys(table, &["command", "args", "startup_timeout_sec", "env"])
        || table.get("command").and_then(Item::as_str) != Some(expected.command.as_str())
        || table.get("startup_timeout_sec").and_then(Item::as_integer)
            != Some(STARTUP_TIMEOUT_SECONDS)
    {
        return false;
    }
    let args_match = table
        .get("args")
        .and_then(Item::as_array)
        .is_some_and(|args| {
            args.len() == 1 && args.get(0).and_then(Value::as_str) == Some(argument)
        });
    let env_match = table
        .get("env")
        .and_then(Item::as_table_like)
        .is_some_and(|env| {
            has_exact_keys(env, &[budget_key])
                && positive_integer_string(env.get(budget_key).and_then(Item::as_str))
        });
    args_match && env_match
}

fn legacy_fastread_table_is_owned(item: &Item) -> bool {
    let Some(table) = item.as_table_like() else {
        return false;
    };
    if !has_only_keys(table, &["command", "startup_timeout_sec", "enabled", "env"])
        || !["command", "startup_timeout_sec", "env"]
            .iter()
            .all(|key| table.get(key).is_some())
        || table.get("startup_timeout_sec").and_then(Item::as_integer)
            != Some(STARTUP_TIMEOUT_SECONDS)
        || table
            .get("enabled")
            .is_some_and(|enabled| enabled.as_bool() != Some(false))
    {
        return false;
    }
    let command = table
        .get("command")
        .and_then(Item::as_str)
        .map(|command| command.replace('\\', "/").to_ascii_lowercase());
    let command_matches = command.is_some_and(|command| {
        command.ends_with("/.fastread/bin/fastread")
            || command.ends_with("/.fastread/bin/fastread.exe")
    });
    let env_matches = table
        .get("env")
        .and_then(Item::as_table_like)
        .is_some_and(|env| {
            env.get("FASTREAD_TOKEN_BUDGET").is_some()
                && env.iter().all(|(key, value)| {
                    matches!(
                        key,
                        "FASTREAD_TOKEN_BUDGET"
                            | "FASTREAD_READ_TOKEN_BUDGET"
                            | "FASTREAD_GREP_TOKEN_BUDGET"
                            | "FASTREAD_GLOB_TOKEN_BUDGET"
                    ) && positive_integer_string(value.as_str())
                })
        });
    command_matches && env_matches
}

fn has_exact_keys(table: &dyn toml_edit::TableLike, allowed: &[&str]) -> bool {
    table.len() == allowed.len() && table.iter().all(|(key, _)| allowed.contains(&key))
}

fn has_only_keys(table: &dyn toml_edit::TableLike, allowed: &[&str]) -> bool {
    table.iter().all(|(key, _)| allowed.contains(&key))
}

fn positive_integer_string(value: Option<&str>) -> bool {
    value
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|value| value > 0)
}

fn server_args(expected: &ExpectedConfig) -> Vec<String> {
    vec![
        "serve".to_string(),
        "--tools".to_string(),
        expected.enabled_tools.names().join(","),
    ]
}

fn insert_tool_budget(table: &mut Table, key: &str, budget: Option<usize>) {
    if let Some(budget) = budget {
        table.insert(key, value(budget.to_string()));
    }
}

fn check_env(
    env: &dyn toml_edit::TableLike,
    expected: &ExpectedConfig,
    global: usize,
    drift: &mut Vec<String>,
) {
    check_env_value(env, "FASTCTX_TOKEN_BUDGET", Some(global), drift);
    for (tool, key, budget) in expected_tool_budgets(expected) {
        check_env_value(
            env,
            key,
            expected
                .enabled_tools
                .contains(tool)
                .then_some(budget)
                .flatten(),
            drift,
        );
    }
}

fn expected_tool_budgets(
    expected: &ExpectedConfig,
) -> [(&'static str, &'static str, Option<usize>); 5] {
    let global = expected.fastctx_budget;
    [
        (
            "inspect_local_file",
            "FASTCTX_READ_TOKEN_BUDGET",
            expected.tool_budgets.read.resolve(global),
        ),
        (
            "grep",
            "FASTCTX_GREP_TOKEN_BUDGET",
            expected.tool_budgets.grep.resolve(global),
        ),
        (
            "glob",
            "FASTCTX_GLOB_TOKEN_BUDGET",
            expected.tool_budgets.glob.resolve(global),
        ),
        (
            "run",
            "FASTCTX_RUN_TOKEN_BUDGET",
            expected.tool_budgets.run.resolve(global),
        ),
        (
            "job_output",
            "FASTCTX_JOB_OUTPUT_TOKEN_BUDGET",
            expected.tool_budgets.job_output.resolve(global),
        ),
    ]
}

fn check_env_value(
    env: &dyn toml_edit::TableLike,
    key: &str,
    expected: Option<usize>,
    drift: &mut Vec<String>,
) {
    let actual = env.get(key).and_then(Item::as_str);
    let expected_string = expected.map(|value| value.to_string());
    if actual != expected_string.as_deref() {
        drift.push(format!("mcp_servers.fastctx.env.{key}"));
    }
}
