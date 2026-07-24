//! Ownership-aware Codex TOML editing with `toml_edit` preserving all unowned content.

use crate::control::settings::{Tier, ToolBudgets};
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
    /// Five long-output tools' relative budgets.
    pub tool_budgets: ToolBudgets,
    /// Whether Apply should publish the optional shell tool group.
    pub fastshell_enabled: bool,
}

/// Conflict on a shared key in an Apply plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenLimitConflict {
    /// Current user setting.
    pub current: i64,
    /// Setting required by the FastCtx tier.
    pub requested: i64,
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
}

/// Parses Codex TOML and produces the post-Apply bytes.
pub fn apply(original: &[u8], expected: &ExpectedConfig) -> Result<ApplyEdit, String> {
    let (migrated, legacy) = strip_owned_legacy_servers(original, expected)?;
    let mut document = parse(&migrated)?;
    let requested_limit = expected.tier.host_limit();
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
    if let Some(existing) = mcp_servers.get("fastctx").and_then(Item::as_table) {
        *fastctx_table.decor_mut() = existing.decor().clone();
    }
    mcp_servers.insert("fastctx", Item::Table(fastctx_table));

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
            reconcile_namespace(array, FASTCTX_NAMESPACE, true);
        }
        None => {
            let mut array = Array::new();
            array.push(FASTCTX_NAMESPACE);
            code_mode.insert(
                "direct_only_tool_namespaces",
                Item::Value(Value::Array(array)),
            );
        }
    }
    set_integer(&mut document, "tool_output_token_limit", requested_limit)?;

    Ok(ApplyEdit {
        bytes: document.to_string().into_bytes(),
        previous_token_limit_present,
        previous_token_limit,
        conflict,
    })
}

/// Removes FastCtx-owned configuration in reverse; the shared token key is restored only when explicitly allowed.
pub fn unapply(
    original: &[u8],
    restore_token_limit: bool,
    previous_token_limit_present: bool,
    previous_token_limit: Option<i64>,
) -> Result<Vec<u8>, String> {
    let mut document = parse(original)?;
    if document.get("mcp_servers").is_some() {
        let emptied = {
            let mcp_servers = document
                .get_mut("mcp_servers")
                .and_then(Item::as_table_mut)
                .ok_or_else(|| {
                    "Codex config key mcp_servers is not a table. Repair it manually and retry."
                        .to_string()
                })?;
            mcp_servers.remove("fastctx");
            mcp_servers.is_empty()
        };
        // Remove an mcp_servers table created solely by Apply so no empty shell remains;
        // this also keeps drift-free reverse removal byte-exact with Apply (2026-07-12).
        if emptied {
            document.remove("mcp_servers");
        }
    }

    if document.get("features").is_some() {
        let emptied = {
            let features = document
                .get_mut("features")
                .and_then(Item::as_table_mut)
                .ok_or_else(|| {
                    "Codex config key features is not a table. Repair it manually and retry."
                        .to_string()
                })?;
            let mut remove_code_mode = false;
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
                    for index in (0..array.len()).rev() {
                        if array.get(index).and_then(Value::as_str) == Some(FASTCTX_NAMESPACE) {
                            remove_array_index_preserving_trailing(array, index);
                        }
                    }
                    if array.is_empty() {
                        code_mode.remove("direct_only_tool_namespaces");
                    }
                }
                remove_code_mode = code_mode.is_empty();
            }
            if remove_code_mode {
                features.remove("code_mode");
            }
            features.is_empty()
        };
        // Likewise remove a features table created solely by Apply after code_mode is removed.
        if emptied {
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
        expected.tier.host_limit(),
        expected.tier.fastctx_budget(),
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
                Ok(env) => check_env(env, expected, fastctx_budget, &mut drift),
                Err(_) => drift.push("mcp_servers.fastctx.env".to_string()),
            }
            let actual_args = table.get("args").and_then(Item::as_array);
            let expected_args = server_args(expected);
            let args_match = actual_args.is_some_and(|args| {
                args.len() == expected_args.len()
                    && args
                        .iter()
                        .zip(expected_args.iter())
                        .all(|(actual, expected)| actual.as_str() == Some(*expected))
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
    let matching = array
        .iter()
        .enumerate()
        .filter_map(|(index, value)| (value.as_str() == Some(namespace)).then_some(index))
        .collect::<Vec<_>>();
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
    let global = expected.tier.fastctx_budget();
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
    insert_tool_budget(
        &mut env,
        "FASTCTX_READ_TOKEN_BUDGET",
        expected.tool_budgets.read.resolve(global),
    );
    insert_tool_budget(
        &mut env,
        "FASTCTX_GREP_TOKEN_BUDGET",
        expected.tool_budgets.grep.resolve(global),
    );
    insert_tool_budget(
        &mut env,
        "FASTCTX_GLOB_TOKEN_BUDGET",
        expected.tool_budgets.glob.resolve(global),
    );
    insert_tool_budget(
        &mut env,
        "FASTCTX_RUN_TOKEN_BUDGET",
        expected.tool_budgets.run.resolve(global),
    );
    insert_tool_budget(
        &mut env,
        "FASTCTX_JOB_OUTPUT_TOKEN_BUDGET",
        expected.tool_budgets.job_output.resolve(global),
    );
    table.insert("env", Item::Table(env));
    table
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

fn server_args(expected: &ExpectedConfig) -> Vec<&'static str> {
    let mut args = vec!["serve"];
    if expected.fastshell_enabled {
        args.push("--enable-shell");
    }
    args
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
    check_env_value(
        env,
        "FASTCTX_READ_TOKEN_BUDGET",
        expected.tool_budgets.read.resolve(global),
        drift,
    );
    check_env_value(
        env,
        "FASTCTX_GREP_TOKEN_BUDGET",
        expected.tool_budgets.grep.resolve(global),
        drift,
    );
    check_env_value(
        env,
        "FASTCTX_GLOB_TOKEN_BUDGET",
        expected.tool_budgets.glob.resolve(global),
        drift,
    );
    check_env_value(
        env,
        "FASTCTX_RUN_TOKEN_BUDGET",
        expected.tool_budgets.run.resolve(global),
        drift,
    );
    check_env_value(
        env,
        "FASTCTX_JOB_OUTPUT_TOKEN_BUDGET",
        expected.tool_budgets.job_output.resolve(global),
        drift,
    );
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

#[cfg(test)]
mod tests {
    use super::{ExpectedConfig, TOOL_TIMEOUT_SECONDS, apply, drift, drift_applied, unapply};
    use crate::control::settings::{Tier, ToolBudgetLevel, ToolBudgets};

    fn expected() -> ExpectedConfig {
        ExpectedConfig {
            command: "C:/Users/test/.fastctx/bin/fastctx.exe".to_string(),
            tier: Tier::Standard,
            tool_budgets: ToolBudgets {
                read: ToolBudgetLevel::Inherit,
                grep: ToolBudgetLevel::Percent50,
                glob: ToolBudgetLevel::Percent25,
                run: ToolBudgetLevel::Inherit,
                job_output: ToolBudgetLevel::Inherit,
            },
            fastshell_enabled: false,
        }
    }

    #[test]
    fn apply_preserves_unowned_bytes_and_array_order() {
        let original = concat!(
            "# user heading\n",
            "tool_output_token_limit = 10000 # keep this comment\n",
            "\n",
            "[mcp_servers.other]\n",
            "command = 'other'\n",
            "\n",
            "[features.code_mode]\n",
            "direct_only_tool_namespaces = [ 'alpha', 'omega' ]\n",
        );
        let edit = apply(original.as_bytes(), &expected()).unwrap();
        let output = std::str::from_utf8(&edit.bytes).unwrap();
        assert!(output.contains("# user heading"));
        assert!(output.contains("command = 'other'"));
        let alpha = output.find("alpha").unwrap();
        let omega = output.find("omega").unwrap();
        let fastctx = output.find("mcp__fastctx").unwrap();
        assert!(alpha < omega && omega < fastctx, "{output}");
        assert!(output.contains("tool_output_token_limit = 20000 # keep this comment"));
        assert!(output.contains("tool_timeout_sec = 300"));
        assert_eq!(edit.conflict.unwrap().current, 10_000);
        assert!(drift(&edit.bytes, &expected()).unwrap().is_empty());
    }

    #[test]
    fn apply_is_idempotent() {
        let first = apply(b"", &expected()).unwrap().bytes;
        let second = apply(&first, &expected()).unwrap().bytes;
        assert_eq!(first, second);
    }

    #[test]
    fn unapply_only_removes_owned_entries_and_keeps_shared_limit_by_default() {
        let applied = apply(
            b"[features.code_mode]\ndirect_only_tool_namespaces = [\"other\"]\n",
            &expected(),
        )
        .unwrap();
        let removed = unapply(
            &applied.bytes,
            false,
            applied.previous_token_limit_present,
            applied.previous_token_limit,
        )
        .unwrap();
        let output = std::str::from_utf8(&removed).unwrap();
        assert!(!output.contains("mcp__fastctx"));
        assert!(output.contains("other"));
        assert!(output.contains("tool_output_token_limit = 20000"));
        assert!(!output.contains("[mcp_servers.fastctx]"));
    }

    #[test]
    fn unapply_rejects_a_drifted_namespace_type_instead_of_claiming_success() {
        let error = unapply(
            b"[features.code_mode]\ndirect_only_tool_namespaces = 'broken'\n",
            false,
            false,
            None,
        )
        .unwrap_err();
        assert!(error.contains("is not an array"));
    }

    #[test]
    fn namespace_add_then_remove_preserves_existing_element_order_and_spacing() {
        let original =
            b"[features.code_mode]\ndirect_only_tool_namespaces = [ 'alpha', 'omega' ]\n";
        let applied = apply(original, &expected()).unwrap();
        let removed = unapply(
            &applied.bytes,
            false,
            applied.previous_token_limit_present,
            applied.previous_token_limit,
        )
        .unwrap();
        let output = std::str::from_utf8(&removed).unwrap();
        assert!(
            output.contains("direct_only_tool_namespaces = [ 'alpha', 'omega' ]"),
            "{output}"
        );
    }

    #[test]
    fn token_restore_refuses_a_non_integer_key() {
        // The Unapply restoration path reuses set_integer's type guard and refuses a table-valued tool_output_token_limit.
        let error = unapply(
            b"[tool_output_token_limit]\nvalue = 1\n",
            true,
            true,
            Some(10_000),
        )
        .unwrap_err();
        assert!(error.contains("is not an integer"));
    }

    #[test]
    fn removing_the_only_namespace_deletes_the_empty_code_mode_shell() {
        let removed = unapply(
            b"[features.code_mode]\ndirect_only_tool_namespaces = [\"mcp__fastctx\"]\n",
            false,
            false,
            None,
        )
        .unwrap();
        let output = std::str::from_utf8(&removed).unwrap();
        assert!(!output.contains("code_mode"), "{output}");
        assert!(!output.contains("direct_only_tool_namespaces"), "{output}");
    }

    #[test]
    fn one_server_args_and_namespace_follow_the_shell_toggle() {
        for fastshell in [false, true] {
            let mut expected = expected();
            expected.fastshell_enabled = fastshell;
            let bytes = apply(b"", &expected).unwrap().bytes;
            let source = std::str::from_utf8(&bytes).unwrap();
            assert!(source.contains("[mcp_servers.fastctx]"));
            let document = source.parse::<toml_edit::DocumentMut>().unwrap();
            let args = document["mcp_servers"]["fastctx"]["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect::<Vec<_>>();
            let mut expected_args = vec!["serve"];
            if fastshell {
                expected_args.push("--enable-shell");
            }
            assert_eq!(args, expected_args);
            assert_eq!(source.matches("mcp__fastctx").count(), 1);
            assert!(!source.contains("approval_mode"));
            assert_eq!(
                document["mcp_servers"]["fastctx"]["tool_timeout_sec"].as_integer(),
                Some(TOOL_TIMEOUT_SECONDS)
            );
            assert!(drift(&bytes, &expected).unwrap().is_empty());
        }
    }

    #[test]
    fn legacy_receipt_limits_and_missing_tool_timeout_do_not_report_false_drift() {
        let mut expected = expected();
        expected.tier = Tier::High;
        let legacy = concat!(
            "tool_output_token_limit = 16000\n",
            "[mcp_servers.fastctx]\n",
            "command = \"C:/Users/test/.fastctx/bin/fastctx.exe\"\n",
            "args = [\"serve\"]\n",
            "startup_timeout_sec = 120\n",
            "[mcp_servers.fastctx.env]\n",
            "FASTCTX_TOKEN_BUDGET = \"13600\"\n",
            "FASTCTX_GREP_TOKEN_BUDGET = \"6800\"\n",
            "FASTCTX_GLOB_TOKEN_BUDGET = \"3400\"\n",
            "[features.code_mode]\n",
            "direct_only_tool_namespaces = [\"mcp__fastctx\"]\n",
        );

        assert!(
            drift_applied(legacy.as_bytes(), &expected, 16_000, 13_600, None)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            drift_applied(
                legacy.as_bytes(),
                &expected,
                16_000,
                13_600,
                Some(TOOL_TIMEOUT_SECONDS),
            )
            .unwrap(),
            vec!["mcp_servers.fastctx.tool_timeout_sec"]
        );
    }

    #[test]
    fn reapply_removes_the_deprecated_edit_flag_and_tracks_the_shell_toggle() {
        let mut enabled = expected();
        enabled.fastshell_enabled = true;
        let mut applied = apply(
            b"[mcp_servers.other]\ncommand='other'\n[features.code_mode]\ndirect_only_tool_namespaces=['other']\n",
            &enabled,
        )
        .unwrap()
        .bytes;
        let source = std::str::from_utf8(&applied).unwrap();
        applied = source
            .replace(
                "args = [\"serve\", \"--enable-shell\"]",
                "args = [\"serve\", \"--enable-shell\", \"--enable-edit\"]",
            )
            .into_bytes();
        let disabled = expected();
        applied = apply(&applied, &disabled).unwrap().bytes;
        let source = std::str::from_utf8(&applied).unwrap();
        assert!(source.contains("[mcp_servers.other]"));
        assert!(source.contains("'other'"));
        assert!(source.contains("args = [\"serve\"]"));
        assert!(!source.contains("--enable-shell"));
        assert!(!source.contains("--enable-edit"));
        assert!(drift(&applied, &disabled).unwrap().is_empty());
    }

    #[test]
    fn apply_migrates_only_exact_owned_legacy_servers_and_namespaces() {
        let original = concat!(
            "[mcp_servers.fastread]\n",
            "command = 'C:/Users/test/.fastread/bin/fastread.exe'\n",
            "startup_timeout_sec = 120\n",
            "enabled = false\n",
            "[mcp_servers.fastread.env]\n",
            "FASTREAD_TOKEN_BUDGET = '8500'\n",
            "FASTREAD_GLOB_TOKEN_BUDGET = '2100'\n",
            "\n",
            "[mcp_servers.fastctx]\n",
            "command = 'C:/Users/test/.fastctx/bin/fastctx.exe'\n",
            "startup_timeout_sec = 120\n",
            "[mcp_servers.fastctx.env]\n",
            "FASTCTX_TOKEN_BUDGET = '8500'\n",
            "\n",
            "[mcp_servers.fastshell]\n",
            "command = 'C:/Users/test/.fastctx/bin/fastctx.exe'\n",
            "args = ['shell-serve']\n",
            "startup_timeout_sec = 120\n",
            "[mcp_servers.fastshell.env]\n",
            "FASTSHELL_TOKEN_BUDGET = '8500'\n",
            "\n",
            "[mcp_servers.fastedit]\n",
            "command = 'C:/Users/test/.fastctx/bin/fastctx.exe'\n",
            "args = ['edit-serve']\n",
            "startup_timeout_sec = 120\n",
            "[mcp_servers.fastedit.env]\n",
            "FASTEDIT_TOKEN_BUDGET = '8500'\n",
            "\n",
            "[mcp_servers.user_owned]\n",
            "command = 'keep-me'\n",
            "\n",
            "[features.code_mode]\n",
            "direct_only_tool_namespaces = ['other', 'mcp__fastread', 'mcp__fastctx', 'mcp__fastshell', 'mcp__fastedit']\n",
        );
        let mut expected = expected();
        expected.fastshell_enabled = true;
        let output = apply(original.as_bytes(), &expected).unwrap().bytes;
        let source = std::str::from_utf8(&output).unwrap();
        assert!(!source.contains("[mcp_servers.fastread]"));
        assert!(!source.contains("[mcp_servers.fastshell]"));
        assert!(!source.contains("[mcp_servers.fastedit]"));
        assert!(source.contains("[mcp_servers.user_owned]"));
        assert!(source.contains("command = 'keep-me'"));
        assert!(source.contains("args = [\"serve\", \"--enable-shell\"]"));
        assert!(!source.contains("--enable-edit"));
        assert_eq!(source.matches("mcp__fastctx").count(), 1);
        for legacy in ["mcp__fastread", "mcp__fastshell", "mcp__fastedit"] {
            assert!(
                !source.contains(legacy),
                "{legacy} survived migration:\n{source}"
            );
        }
    }

    #[test]
    fn migration_preserves_legacy_named_tables_after_user_drift() {
        let original = concat!(
            "[mcp_servers.fastshell]\n",
            "command = 'C:/Users/test/.fastctx/bin/fastctx.exe'\n",
            "args = ['custom-shell']\n",
            "startup_timeout_sec = 120\n",
            "[mcp_servers.fastshell.env]\n",
            "FASTSHELL_TOKEN_BUDGET = '8500'\n",
            "\n",
            "[features.code_mode]\n",
            "direct_only_tool_namespaces = ['mcp__fastshell']\n",
        );
        let output = apply(original.as_bytes(), &expected()).unwrap().bytes;
        let source = std::str::from_utf8(&output).unwrap();
        assert!(source.contains("[mcp_servers.fastshell]"));
        assert!(source.contains("args = ['custom-shell']"));
        assert!(source.contains("mcp__fastshell"));
        assert!(source.contains("mcp__fastctx"));
    }
}
