//! Evidence from the nine released 0.x installers. Frozen guidance comes
//! from each release tag's `src/control/agents.rs`, not later constants
//! named after those releases (the later "v0.2.4" constant is different).

use std::collections::BTreeMap;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use toml_edit::{Item, Value};

use crate::codex::Limit;
use crate::guidance::{self, BEGIN, END, OLD_BEGIN, OLD_END, marker_range};
use crate::source::{Source, comma_in, edit, splice};
use crate::{Error, File, Notice, check_path};

/// A released installer's `[applied]` record, bound to both original paths.
/// Parsing the settings bytes does not access the named files.
#[derive(Clone, Debug)]
pub struct Receipt {
    record: AppliedRecord,
}

#[derive(Clone, Debug, Deserialize)]
struct ManagedFile {
    path: String,
    original_existed: Option<bool>,
    applied_sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
struct AppliedRecord {
    version: String,
    command: String,
    tool_output_token_limit: i64,
    tool_timeout_sec: Option<i64>,
    previous_token_limit_present: bool,
    previous_token_limit: Option<i64>,
    fastctx_token_budget: u64,
    #[serde(default)]
    tool_budgets: BTreeMap<String, toml::Value>,
    #[serde(default)]
    fastshell_enabled: bool,
    codex_config: ManagedFile,
    codex_agents: ManagedFile,
    agents_contract_id: Option<String>,
    codex_agents_inserted_separator: Option<String>,
}

impl Receipt {
    /// Accepts exactly the released schema and versions. Unknown receipts
    /// fail without quoting potentially unrelated settings in diagnostics.
    pub fn from_bytes(bytes: &[u8]) -> Result<Option<Self>, Error> {
        #[derive(Deserialize)]
        struct Settings {
            schema_version: u32,
            applied: Option<AppliedRecord>,
        }
        let text = std::str::from_utf8(bytes).map_err(|_| Error::at("legacy receipt", "not UTF-8"))?;
        let settings: Settings = toml::from_str(text)
            .map_err(|_| Error::at("legacy receipt", "invalid settings or incomplete applied record"))?;
        if settings.schema_version > 1 {
            return Err(Error::at(
                "legacy receipt",
                format!("unsupported schema {} (expected released schema 0 or 1)", settings.schema_version),
            ));
        }
        let Some(record) = settings.applied else { return Ok(None) };
        if !matches!(
            record.version.as_str(),
            "0.1.0" | "0.1.1" | "0.2.0" | "0.2.1" | "0.2.2" | "0.2.3" | "0.2.4" | "0.2.5" | "0.2.6"
        ) {
            return Err(Error::at("legacy receipt", format!("unsupported released version {}", record.version)));
        }
        if record.previous_token_limit_present != record.previous_token_limit.is_some() {
            return Err(Error::at("legacy receipt", "previous token-limit presence and value disagree"));
        }
        if record.fastctx_token_budget == 0 || record.tool_output_token_limit < 0 {
            return Err(Error::at("legacy receipt", "invalid recorded output budget"));
        }
        if let Some(separator) = &record.codex_agents_inserted_separator
            && !matches!(separator.as_str(), "lf" | "cr-lf" | "lf-lf")
        {
            return Err(Error::at("legacy receipt", "unknown guidance separator"));
        }
        let receipt = Self { record };
        receipt.expected_server()?;
        Ok(Some(receipt))
    }

    pub fn config_path(&self) -> &str {
        &self.record.codex_config.path
    }
    pub fn agents_path(&self) -> &str {
        &self.record.codex_agents.path
    }

    /// The IO caller must first verify that these resolved paths name the
    /// recorded files. Pure editing then uses the same identities as Snapshot.
    pub(crate) fn bind_paths(&mut self, config: String, agents: String) {
        self.record.codex_config.path = config;
        self.record.codex_agents.path = agents;
    }

    fn expected_server(&self) -> Result<toml::Value, Error> {
        let record = &self.record;
        let mut table = toml::Table::new();
        table.insert("command".into(), record.command.clone().into());
        let mut args = vec![toml::Value::String("serve".into())];
        if record.fastshell_enabled {
            args.push("--enable-shell".into());
        }
        table.insert("args".into(), args.into());
        table.insert("startup_timeout_sec".into(), 120.into());
        if let Some(timeout) = record.tool_timeout_sec {
            table.insert("tool_timeout_sec".into(), timeout.into());
        }
        let mut env = toml::Table::new();
        env.insert("FASTCTX_TOKEN_BUDGET".into(), record.fastctx_token_budget.to_string().into());
        for (tool, key) in
            [("read", "READ"), ("grep", "GREP"), ("glob", "GLOB"), ("run", "RUN"), ("job_output", "JOB_OUTPUT")]
        {
            let Some(value) = record.tool_budgets.get(tool) else { continue };
            let percent = percent(value)?;
            if percent != 100 {
                let raw = record
                    .fastctx_token_budget
                    .checked_mul(percent)
                    .and_then(|v| v.checked_add(50))
                    .ok_or_else(|| Error::at("legacy receipt", "budget overflow"))?
                    / 100;
                let rounded =
                    raw.checked_add(50).ok_or_else(|| Error::at("legacy receipt", "budget overflow"))? / 100 * 100;
                // v0.2.3's arbitrary percentages added the minimum hundred;
                // the earlier quarter-share calculation could round to zero.
                let budget = if matches!(record.version.as_str(), "0.1.0" | "0.1.1" | "0.2.0" | "0.2.1" | "0.2.2") {
                    rounded
                } else {
                    rounded.max(100)
                };
                env.insert(format!("FASTCTX_{key}_TOKEN_BUDGET"), budget.to_string().into());
            }
        }
        table.insert("env".into(), env.into());
        Ok(table.into())
    }
}

/// Retires only the old installer's ownership record, preserving other settings
/// and comments byte for byte. Call after migration or withdrawal was planned,
/// and publish in the same transaction as the affected host files.
pub(crate) fn retire(file: File<'_>) -> Result<Vec<u8>, Error> {
    let bytes = file.bytes.unwrap_or_default();
    let source = Source::parse(bytes, file.path)?;
    let edits = source.table_ranges(&["applied"])?.into_iter().map(|range| (range, vec![])).collect();
    let result = splice(bytes, edits)?;
    Source::parse(&result, file.path)?;
    Ok(result)
}

fn percent(value: &toml::Value) -> Result<u64, Error> {
    let number = match value {
        toml::Value::Integer(value) => u64::try_from(*value).ok(),
        toml::Value::String(value) => match value.as_str() {
            "inherit" => Some(100),
            "percent75" => Some(75),
            "percent50" => Some(50),
            "percent25" => Some(25),
            value => value.trim().strip_suffix('%').and_then(|p| p.trim().parse().ok()),
        },
        _ => None,
    };
    number.filter(|v| (1..=100).contains(v)).ok_or_else(|| Error::at("legacy receipt", "invalid per-tool budget share"))
}

pub(crate) struct TakenOver {
    pub config: Vec<u8>,
    pub agents: Option<Vec<u8>>,
    pub config_existed: bool,
    pub agents_existed: bool,
    pub limit: Option<Limit>,
    pub guidance: Option<guidance::Receipt>,
    pub notices: Vec<Notice>,
}

pub(crate) fn take_over(config: File<'_>, agents: File<'_>, receipt: &Receipt) -> Result<TakenOver, Error> {
    check_path(config.path, receipt.config_path())?;
    check_path(agents.path, receipt.agents_path())?;
    let record = &receipt.record;
    let mut notices = vec![Notice {
        location: "legacy receipt".into(),
        reason: "0.x recorded the prior root limit as an integer, not its original spelling, and did not back up overwritten servers, guidance, or namespace entries; only recorded values and surviving user bytes can be recovered".into(),
    }];
    for file in [&record.codex_config, &record.codex_agents] {
        if file.original_existed.is_none() {
            notices.push(Notice {
                location: file.path.clone(),
                reason: "the old receipt omitted original_existed; an empty file will be preserved".into(),
            });
        }
    }
    let original = config.bytes.unwrap_or_default();
    let source = Source::parse(original, config.path)?;
    let mut owned_servers = vec![];
    if let Some(server) = source.item(&["mcp_servers", "fastctx"]) {
        let expected = receipt.expected_server()?;
        if !matches_value(server, &expected) {
            return Err(Error::at(
                format!("{}: mcp_servers.fastctx", config.path),
                "the legacy server differs from its applied record",
            ));
        }
        owned_servers.push("fastctx");
    }
    for name in ["fastread", "fastshell", "fastedit"] {
        if let Some(server) = source.item(&["mcp_servers", name]) {
            if hash(original) == record.codex_config.applied_sha256 && old_server_matches(name, server, &record.command)
            {
                owned_servers.push(name);
            } else {
                notices.push(Notice {
                    location: format!("{}: mcp_servers.{name}", config.path),
                    reason: "the old whole-file hash or frozen shape does not match; the receipt cannot attribute this earlier server or namespace to the old installer, so both are left in place".into(),
                });
            }
        }
    }
    let limit = if let Some(item) = source.item(&["tool_output_token_limit"]) {
        if item.as_integer() == Some(record.tool_output_token_limit) {
            let range = source.span(item.span())?;
            let written = original[range].to_vec();
            let statement = source.assignment(&["tool_output_token_limit"])?;
            let canonical = format!("tool_output_token_limit = {}\n", record.tool_output_token_limit).into_bytes();
            let assignment = (!record.previous_token_limit_present
                && original.get(statement.start..statement.start + canonical.len()) == Some(canonical.as_slice()))
            .then_some(canonical);
            Some(Limit {
                before: record.previous_token_limit.map(|value| value.to_string().into_bytes()),
                written,
                assignment,
            })
        } else {
            notices.push(Notice {
                location: format!("{}: tool_output_token_limit", config.path),
                reason: "changed since the old apply; the old receipt no longer owns it".into(),
            });
            None
        }
    } else {
        None
    };
    let mut config_bytes = original.to_vec();
    for name in &owned_servers {
        let path = ["mcp_servers", *name];
        config_bytes = edit::remove(&Source::parse(&config_bytes, config.path)?, &path)?;
    }
    for name in &owned_servers {
        config_bytes = remove_namespace(&config_bytes, config.path, &format!("mcp__{name}"), &mut notices)?;
    }
    Source::parse(&config_bytes, config.path)?;
    let mut agents_bytes = agents.bytes.map(<[u8]>::to_vec);
    let mut guidance_receipt = None;
    if let Some(bytes) = &mut agents_bytes {
        if let Some(range) = marker_range(bytes, OLD_BEGIN, OLD_END, agents.path)? {
            if hash(agents.bytes.unwrap_or_default()) == record.codex_agents.applied_sha256
                && &bytes[range.clone()] == include_bytes!("legacy_guidance/fastread.txt")
            {
                // These markers predate 0.x's separator receipts. No surrounding
                // bytes can be attributed to that earlier installer.
                bytes.drain(range);
            } else {
                notices.push(Notice {
                    location: format!("{}: fastread block", agents.path),
                    reason: "the old whole-file hash or frozen body does not match; ownership of this earlier block is unproven, so it is left in place".into(),
                });
            }
        }
        if let Some(range) = marker_range(bytes, BEGIN, END, agents.path)? {
            if !known_guidance(&bytes[range.clone()], record) {
                return Err(Error::at(
                    format!("{}: fastctx block", agents.path),
                    "the legacy guidance differs from its released body",
                ));
            }
            let whole_matches = hash(agents.bytes.unwrap_or_default()) == record.codex_agents.applied_sha256;
            let prefix =
                if whole_matches { separator_bytes(record.codex_agents_inserted_separator.as_deref()) } else { vec![] };
            if !whole_matches && record.codex_agents_inserted_separator.is_some() {
                notices.push(Notice {
                    location: agents.path.into(),
                    reason: "the old whole-file hash changed; the legacy leading separator cannot be reclaimed".into(),
                });
            }
            let suffix = if bytes[range.end..].starts_with(b"\n") { b"\n".to_vec() } else { vec![] };
            guidance_receipt = Some(guidance::Receipt {
                path: agents.path.into(),
                original_existed: record.codex_agents.original_existed.unwrap_or(true),
                block: bytes[range].to_vec(),
                prefix,
                suffix,
            });
        }
    }
    Ok(TakenOver {
        config: config_bytes,
        agents: agents_bytes,
        config_existed: record.codex_config.original_existed.unwrap_or(true),
        agents_existed: record.codex_agents.original_existed.unwrap_or(true),
        limit,
        guidance: guidance_receipt,
        notices,
    })
}

fn matches_value(item: &Item, expected: &toml::Value) -> bool {
    match expected {
        toml::Value::Table(expected) => item.as_table_like().is_some_and(|table| {
            table.len() == expected.len()
                && expected.iter().all(|(key, value)| table.get(key).is_some_and(|item| matches_value(item, value)))
        }),
        toml::Value::Array(expected) => item.as_array().is_some_and(|array| {
            array.len() == expected.len()
                && array
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| matches_value(&Item::Value(actual.clone()), expected))
        }),
        toml::Value::String(value) => item.as_str() == Some(value),
        toml::Value::Integer(value) => item.as_integer() == Some(*value),
        toml::Value::Boolean(value) => item.as_bool() == Some(*value),
        _ => false,
    }
}

fn old_server_matches(name: &str, item: &Item, command: &str) -> bool {
    let Some(table) = item.as_table() else { return false };
    if table.get("startup_timeout_sec").and_then(Item::as_integer) != Some(120) {
        return false;
    }
    let Some(env) = table.get("env").and_then(Item::as_table_like) else { return false };
    let positive = |item: &Item| item.as_str().and_then(|s| s.parse::<u64>().ok()).is_some_and(|n| n > 0);
    if name == "fastread" {
        if table.iter().any(|(k, _)| !["command", "startup_timeout_sec", "enabled", "env"].contains(&k))
            || table.get("enabled").is_some_and(|v| v.as_bool() != Some(false))
        {
            return false;
        }
        let Some(command) = table.get("command").and_then(Item::as_str) else { return false };
        let command = command.replace('\\', "/").to_ascii_lowercase();
        (command.ends_with("/.fastread/bin/fastread") || command.ends_with("/.fastread/bin/fastread.exe"))
            && env.contains_key("FASTREAD_TOKEN_BUDGET")
            && env.iter().all(|(key, value)| {
                [
                    "FASTREAD_TOKEN_BUDGET",
                    "FASTREAD_READ_TOKEN_BUDGET",
                    "FASTREAD_GREP_TOKEN_BUDGET",
                    "FASTREAD_GLOB_TOKEN_BUDGET",
                ]
                .contains(&key)
                    && positive(value)
            })
    } else {
        let (argument, budget) = if name == "fastshell" {
            ("shell-serve", "FASTSHELL_TOKEN_BUDGET")
        } else {
            ("edit-serve", "FASTEDIT_TOKEN_BUDGET")
        };
        table.len() == 4
            && table.get("command").and_then(Item::as_str) == Some(command)
            && table
                .get("args")
                .and_then(Item::as_array)
                .is_some_and(|a| a.len() == 1 && a.get(0).and_then(Value::as_str) == Some(argument))
            && env.len() == 1
            && env.get(budget).is_some_and(positive)
    }
}

fn remove_namespace(
    bytes: &[u8],
    location: &str,
    namespace: &str,
    notices: &mut Vec<Notice>,
) -> Result<Vec<u8>, Error> {
    let source = Source::parse(bytes, location)?;
    let path = ["features", "code_mode", "direct_only_tool_namespaces"];
    let Some(item) = source.item(&path) else { return Ok(bytes.to_vec()) };
    let Some(array) = item.as_array() else {
        notices.push(Notice {
            location: format!("{location}: {}", path.join(".")),
            reason: "the legacy namespace list is no longer an array; left it in place".into(),
        });
        return Ok(bytes.to_vec());
    };
    let matching: Vec<_> = array.iter().enumerate().filter(|(_, value)| value.as_str() == Some(namespace)).collect();
    if matching.len() != 1 {
        if !matching.is_empty() {
            notices.push(Notice {
                location: format!("{location}: {namespace}"),
                reason: "duplicate legacy namespace entries cannot be attributed to one insertion; left them in place"
                    .into(),
            });
        }
        return Ok(bytes.to_vec());
    }
    let (index, value) = matching[0];
    let range = source.span(value.span())?;
    if bytes[range.clone()] != *format!("\"{namespace}\"").as_bytes() {
        notices.push(Notice {
            location: format!("{location}: {namespace}"),
            reason: "the namespace token differs from the released writer's spelling; left it in place".into(),
        });
        return Ok(bytes.to_vec());
    }
    let array_range = source.span(array.span())?;
    let next_start = array
        .get(index + 1)
        .map(|v| source.span(v.span()).map(|s| s.start))
        .transpose()?
        .unwrap_or(array_range.end - 1);
    let mut edits = vec![(range.clone(), vec![])];
    if let Some(comma) = comma_in(bytes, range.end..next_start) {
        edits.push((comma..comma + 1, vec![]));
    } else if index > 0 {
        let previous = source.span(array.get(index - 1).expect("previous element").span())?;
        let comma = comma_in(bytes, previous.end..range.start)
            .ok_or_else(|| Error::at(location, "namespace array comma is missing"))?;
        edits.push((comma..comma + 1, vec![]));
    }
    let next = splice(bytes, edits)?;
    Source::parse(&next, location)?;
    Ok(next)
}

fn separator_bytes(separator: Option<&str>) -> Vec<u8> {
    match separator {
        Some("lf") => b"\n".to_vec(),
        Some("cr-lf") => b"\r\n".to_vec(),
        Some("lf-lf") => b"\n\n".to_vec(),
        _ => vec![],
    }
}

fn known_guidance(bytes: &[u8], record: &AppliedRecord) -> bool {
    let variants: [[&[u8]; 2]; 4] = [
        [include_bytes!("legacy_guidance/v010.txt"), include_bytes!("legacy_guidance/v010-shell.txt")],
        [include_bytes!("legacy_guidance/v022.txt"), include_bytes!("legacy_guidance/v022-shell.txt")],
        [include_bytes!("legacy_guidance/v024.txt"), include_bytes!("legacy_guidance/v024-shell.txt")],
        [include_bytes!("legacy_guidance/v025.txt"), include_bytes!("legacy_guidance/v025-shell.txt")],
    ];
    let index = match record.version.as_str() {
        "0.1.0" | "0.1.1" | "0.2.0" | "0.2.1" => 0,
        "0.2.2" | "0.2.3" => 1,
        "0.2.4" => 2,
        _ => 3,
    };
    if record.agents_contract_id.is_some() && record.agents_contract_id.as_deref() != Some("guidance-v4") {
        return false;
    }
    if record.agents_contract_id.is_some() {
        bytes == variants[index][usize::from(record.fastshell_enabled)]
    } else {
        variants[..=index].iter().any(|pair| bytes == pair[usize::from(record.fastshell_enabled)])
    }
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
