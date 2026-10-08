//! Pure installation planning for the caller's selected Codex profile.

use std::ops::Range;

use serde::{Deserialize, Serialize};
use toml_edit::Item;

use crate::guidance;
use crate::legacy;
use crate::plan::CodexPlan;
use crate::source::{Source, comma_in, newline, splice};
use crate::{Error, File, Notice, check_path, finish};

pub(crate) mod tools;

const SERVER: &[&str] = &["mcp_servers", "fastctx"];
const LIMIT: &[&str] = &["tool_output_token_limit"];

/// Caller authorization for changing an existing, insufficient shared limit.
#[derive(Clone, Copy, Debug, Default)]
pub enum Approval {
    #[default]
    Require,
    RaiseOutputLimit,
}

/// Input ownership evidence. A legacy receipt is parsed separately so its
/// profile paths can be inspected before deciding which files to read.
#[derive(Clone, Copy, Debug, Default)]
pub enum Ownership<'a> {
    #[default]
    None,
    Current(&'a Receipt),
    Legacy(&'a legacy::Receipt),
}

/// Persist only after committing both output files. Compare the input bytes
/// again before writing; this pure planner cannot detect concurrent IO.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    version: u32,
    config_path: String,
    config_existed: bool,
    server: Option<Server>,
    pub(crate) limit: Option<Limit>,
    guidance: Option<guidance::Receipt>,
    #[serde(default)]
    tools: Option<tools::Receipt>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Server {
    body: Vec<u8>,
    prefix: Vec<u8>,
    inline: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Limit {
    pub before: Option<Vec<u8>>,
    pub written: Vec<u8>,
    /// Present only when we inserted the entire assignment, including its
    /// terminating newline. Existing assignments own only the value token.
    pub assignment: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct Applied {
    pub config: Vec<u8>,
    pub agents: Vec<u8>,
    pub receipt: Receipt,
    pub notices: Vec<Notice>,
}

#[derive(Clone, Debug)]
pub struct Unapplied {
    pub config: Option<Vec<u8>>,
    pub agents: Option<Vec<u8>>,
    pub receipt: Option<Receipt>,
    pub retained: Vec<Notice>,
}

/// Plans both files atomically in memory. Unowned or edited server/guidance
/// blocks are conflicts. The root limit is shared: sufficient user values
/// stay unowned, and increasing an existing lower value requires approval.
pub fn apply(
    config: File<'_>,
    agents: File<'_>,
    plan: &CodexPlan,
    body: &str,
    ownership: Ownership<'_>,
    approval: Approval,
) -> Result<Applied, Error> {
    validate_plan(plan)?;
    let mut bytes = config.bytes.unwrap_or_default().to_vec();
    let mut agents_bytes = agents.bytes.map(<[u8]>::to_vec);
    let mut notices = vec![];
    let mut legacy_agents_existed = None;
    let mut receipt = Receipt {
        version: 1,
        config_path: config.path.into(),
        config_existed: config.bytes.is_some(),
        server: None,
        limit: None,
        guidance: None,
        tools: None,
    };
    match ownership {
        Ownership::None => {}
        Ownership::Current(old) => {
            old.check(config.path)?;
            receipt = old.clone();
            if let Some(server) = &old.server {
                let source = Source::parse(&bytes, config.path)?;
                server_range(&source, server)?;
            }
        }
        Ownership::Legacy(old) => {
            let migrated = legacy::take_over(config, agents, old)?;
            bytes = migrated.config;
            agents_bytes = migrated.agents;
            receipt.config_existed = migrated.config_existed;
            receipt.limit = migrated.limit;
            receipt.guidance = migrated.guidance;
            legacy_agents_existed = Some(migrated.agents_existed);
            notices = migrated.notices;
        }
    }
    let source = Source::parse(&bytes, config.path)?;
    if let Some(old) = &receipt.limit
        && source.value_range(LIMIT)?.is_none_or(|range| bytes[range] != old.written)
    {
        notices.push(Notice {
            location: format!("{}: tool_output_token_limit", config.path),
            reason: "changed since apply; ownership of the earlier value is relinquished".into(),
        });
    }
    if receipt.server.is_none() && source.item(SERVER).is_some() {
        return Err(Error::at(
            format!("{}: mcp_servers.fastctx", config.path),
            "same-name server has no ownership receipt",
        ));
    }
    let (next, limit) = apply_limit(&bytes, config.path, plan.output_token_limit, receipt.limit.as_ref(), approval)?;
    bytes = next;
    receipt.limit = limit;
    let source = Source::parse(&bytes, config.path)?;
    if let Some(server) = &receipt.server {
        let range = server_range(&source, server)?;
        let replacement = server_body(plan, server.inline, newline(&bytes));
        bytes = splice(&bytes, vec![(range, replacement.clone())])?;
        receipt.server = Some(Server { body: replacement, ..server.clone() });
    } else {
        let (next, server) = insert_server(&source, plan)?;
        bytes = next;
        receipt.server = Some(server);
    }
    Source::parse(&bytes, config.path)?;
    let mut applied =
        guidance::apply(File { path: agents.path, bytes: agents_bytes.as_deref() }, body, receipt.guidance.as_ref())?;
    if let Some(existed) = legacy_agents_existed {
        applied.receipt.original_existed = existed;
    }
    receipt.guidance = Some(applied.receipt);
    let (config, tools, tool_notices) = tools::apply(&bytes, config.path, receipt.tools.as_ref())?;
    bytes = config;
    receipt.tools = Some(tools);
    notices.extend(tool_notices);
    Ok(Applied { config: bytes, agents: applied.bytes, receipt, notices })
}

/// Reverses independent owned changes. Edited portions are retained and
/// reported; their remaining receipt can be kept for a later attempt.
pub fn unapply(config: File<'_>, agents: File<'_>, receipt: Option<&Receipt>) -> Result<Unapplied, Error> {
    let Some(receipt) = receipt else {
        return Ok(Unapplied {
            config: config.bytes.map(<[u8]>::to_vec),
            agents: agents.bytes.map(<[u8]>::to_vec),
            receipt: None,
            retained: vec![],
        });
    };
    receipt.check(config.path)?;
    let mut remaining = receipt.clone();
    let mut retained = vec![];
    let mut bytes = config.bytes.map(<[u8]>::to_vec);
    if let Some(current) = &mut bytes {
        if let Some(tools) = remaining.tools.take() {
            let (next, notices) = tools::unapply(current, config.path, &tools)?;
            *current = next;
            retained.extend(notices);
        }
        if let Some(server) = &receipt.server {
            match remove_server(current, config.path, server) {
                Ok(next) => {
                    *current = next;
                    remaining.server = None;
                }
                Err(error) => retained.push(Notice {
                    location: format!("{}: mcp_servers.fastctx", config.path),
                    reason: error.to_string(),
                }),
            }
        }
        if let Some(limit) = &receipt.limit {
            match restore_limit(current, config.path, limit) {
                Ok(next) => {
                    *current = next;
                    remaining.limit = None;
                }
                Err(error) => retained.push(Notice {
                    location: format!("{}: tool_output_token_limit", config.path),
                    reason: error.to_string(),
                }),
            }
        }
    } else {
        remaining.server = None;
        remaining.limit = None;
        remaining.tools = None;
    }
    let guidance = guidance::unapply(agents, receipt.guidance.as_ref())?;
    remaining.guidance = guidance.receipt;
    retained.extend(guidance.retained);
    let bytes = bytes.and_then(|bytes| finish(bytes, receipt.config_existed));
    let still_owned = remaining.server.is_some() || remaining.limit.is_some() || remaining.guidance.is_some();
    Ok(Unapplied { config: bytes, agents: guidance.bytes, receipt: still_owned.then_some(remaining), retained })
}

/// Withdraws a released install using the same ownership checks as migration,
/// without first installing the World or requiring approval to raise a limit.
pub fn unapply_legacy(config: File<'_>, agents: File<'_>, old: &legacy::Receipt) -> Result<Unapplied, Error> {
    let migrated = legacy::take_over(config, agents, old)?;
    let receipt = Receipt {
        version: 1,
        config_path: config.path.into(),
        config_existed: migrated.config_existed,
        server: None,
        limit: migrated.limit,
        guidance: migrated.guidance,
        tools: None,
    };
    let mut result = unapply(
        File { path: config.path, bytes: Some(&migrated.config) },
        File { path: agents.path, bytes: migrated.agents.as_deref() },
        Some(&receipt),
    )?;
    result.retained.extend(migrated.notices);
    Ok(result)
}

impl Receipt {
    fn check(&self, path: &str) -> Result<(), Error> {
        check_path(path, &self.config_path)?;
        if self.version != 1 {
            return Err(Error::at("receipt", format!("unsupported version {}", self.version)));
        }
        Ok(())
    }
}

fn validate_plan(plan: &CodexPlan) -> Result<(), Error> {
    if !plan.command.is_absolute() || plan.command.to_str().is_none() {
        return Err(Error::at("CodexPlan.command", "expected an absolute UTF-8 path"));
    }
    for (name, value) in [
        ("output_token_limit", plan.output_token_limit),
        ("startup_timeout_sec", plan.startup_timeout_sec),
        ("tool_timeout_sec", plan.tool_timeout_sec),
    ] {
        if i64::try_from(value).is_err() || value == 0 {
            return Err(Error::at(format!("CodexPlan.{name}"), "expected a positive TOML integer"));
        }
    }
    Ok(())
}

/// The server entry. Its tools are never deferred: a model whose tools are
/// deferred sees only their names, and `run` takes the place of a shell whose
/// description it always saw. In Code Mode they stay callable from scripts.
fn server_body(plan: &CodexPlan, inline: bool, nl: &str) -> Vec<u8> {
    let command = toml_edit::Value::from(plan.command.to_str().expect("validated path")).to_string();
    let args = toml_edit::Array::from_iter(plan.args.iter().cloned()).to_string();
    let tools = ["run", "apply_patch", "ask_colleague", "message_colleague"];
    let omit = r#"omit_tools_from = ["deferred"]"#;
    if inline {
        let limits = tools
            .iter()
            .map(|name| format!("{name} = {{ output_token_limit = {} }}", plan.output_token_limit))
            .collect::<Vec<_>>()
            .join(", ");
        format!("fastctx = {{ command = {command}, args = {args}, startup_timeout_sec = {}, tool_timeout_sec = {}, {omit}, tools = {{ {limits} }} }}", plan.startup_timeout_sec, plan.tool_timeout_sec).into_bytes()
    } else {
        let limits = tools
            .iter()
            .map(|name| {
                format!(
                    "{nl}[mcp_servers.fastctx.tools.{name}]{nl}output_token_limit = {}{nl}",
                    plan.output_token_limit
                )
            })
            .collect::<String>();
        format!("[mcp_servers.fastctx]{nl}command = {command}{nl}args = {args}{nl}startup_timeout_sec = {}{nl}tool_timeout_sec = {}{nl}{omit}{nl}{limits}", plan.startup_timeout_sec, plan.tool_timeout_sec).into_bytes()
    }
}

fn insert_server(source: &Source<'_>, plan: &CodexPlan) -> Result<(Vec<u8>, Server), Error> {
    let (at, prefix, inline) = match source.item(&["mcp_servers"]) {
        Some(Item::Value(toml_edit::Value::InlineTable(table))) => {
            let range = source.span(table.span())?;
            let last = table
                .get_values()
                .into_iter()
                .map(|(_, value)| source.span(value.span()))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(|s| s.end)
                .max();
            (last.unwrap_or(range.start + 1), if last.is_some() { b", ".to_vec() } else { vec![] }, true)
        }
        Some(item) if !item.is_table() => return Err(Error::at("mcp_servers", "expected a table")),
        _ => (source.bytes.len(), guidance::separator(source.bytes), false),
    };
    let body = server_body(plan, inline, newline(source.bytes));
    let mut inserted = prefix.clone();
    inserted.extend_from_slice(&body);
    let bytes = splice(source.bytes, vec![(at..at, inserted)])?;
    Source::parse(&bytes, "config.toml")?;
    Ok((bytes, Server { body, prefix, inline }))
}

fn server_range(source: &Source<'_>, server: &Server) -> Result<Range<usize>, Error> {
    let spans = if server.inline {
        if source.item(SERVER).is_none() { vec![] } else { vec![source.assignment(SERVER)?] }
    } else {
        source.table_ranges(SERVER)?
    };
    let start = spans.first().ok_or_else(|| Error::at("mcp_servers.fastctx", "the recorded server is missing"))?.start;
    let end = start + server.body.len();
    if source.bytes.get(start..end) != Some(server.body.as_slice()) || spans.iter().any(|s| s.end > end) {
        return Err(Error::at("mcp_servers.fastctx", "the server differs from the recorded bytes"));
    }
    Ok(start..end)
}

fn remove_server(bytes: &[u8], location: &str, server: &Server) -> Result<Vec<u8>, Error> {
    let source = Source::parse(bytes, location)?;
    if source.item(SERVER).is_none() {
        return Ok(bytes.to_vec());
    }
    let mut range = server_range(&source, server)?;
    if bytes[..range.start].ends_with(&server.prefix) {
        range.start -= server.prefix.len();
    } else if !server.prefix.is_empty() {
        return Err(Error::at("mcp_servers.fastctx", "the server's inserted separator was changed"));
    }
    let mut edits = vec![];
    if server.inline && server.prefix.is_empty() {
        let parent = source
            .item(&["mcp_servers"])
            .and_then(Item::as_inline_table)
            .ok_or_else(|| Error::at("mcp_servers", "the inline parent table was changed"))?;
        let siblings = parent
            .get_values()
            .into_iter()
            .filter(|(keys, _)| keys[0].get() != "fastctx")
            .map(|(keys, value)| Ok(source.span(keys[0].span())?.start..source.span(value.span())?.end))
            .collect::<Result<Vec<_>, _>>()?;
        let next = siblings.iter().filter(|span| span.start > range.end).map(|s| s.start).min();
        let previous = siblings.iter().filter(|span| span.end < range.start).map(|s| s.end).max();
        // A sibling added after installation introduces a delimiter attached
        // to our member. Remove that delimiter, keeping its trivia and sibling.
        let comma = if let Some(next) = next {
            comma_in(bytes, range.end..next)
        } else if let Some(previous) = previous {
            comma_in(bytes, previous..range.start)
        } else {
            None
        };
        if let Some(comma) = comma {
            edits.push((comma..comma + 1, vec![]));
        }
    }
    edits.push((range, vec![]));
    let next = splice(bytes, edits)?;
    Source::parse(&next, location)?;
    Ok(next)
}

fn apply_limit(
    bytes: &[u8],
    location: &str,
    requested: u64,
    old: Option<&Limit>,
    approval: Approval,
) -> Result<(Vec<u8>, Option<Limit>), Error> {
    let source = Source::parse(bytes, location)?;
    let range = source.value_range(LIMIT)?;
    let current = source
        .item(LIMIT)
        .map(|i| i.as_integer().ok_or_else(|| Error::at("tool_output_token_limit", "expected an integer")))
        .transpose()?;
    let owned = old.filter(|old| range.as_ref().is_some_and(|r| bytes[r.clone()] == old.written));
    if current.is_some_and(|value| value >= requested as i64) {
        return Ok((bytes.to_vec(), owned.cloned()));
    }
    if let Some(current) = current
        && !matches!(approval, Approval::RaiseOutputLimit)
    {
        return Err(Error::TokenLimitApproval { location: location.into(), current, requested });
    }
    let written = requested.to_string().into_bytes();
    let (edit, record) = if let Some(range) = range {
        let mut record = owned.cloned().unwrap_or(Limit {
            before: Some(bytes[range.clone()].to_vec()),
            written: vec![],
            assignment: None,
        });
        if let Some(assignment) = &mut record.assignment {
            let text = format!("tool_output_token_limit = {}{}", requested, newline(assignment)).into_bytes();
            *assignment = text;
        }
        record.written = written.clone();
        ((range, written), record)
    } else {
        let assignment = format!("tool_output_token_limit = {}{}", requested, newline(bytes)).into_bytes();
        let record = Limit { before: None, written, assignment: Some(assignment.clone()) };
        ((source.root_offset()..source.root_offset(), assignment), record)
    };
    Ok((splice(bytes, vec![edit])?, Some(record)))
}

pub(crate) fn restore_limit(bytes: &[u8], location: &str, limit: &Limit) -> Result<Vec<u8>, Error> {
    let source = Source::parse(bytes, location)?;
    let Some(range) = source.value_range(LIMIT)? else {
        return Ok(bytes.to_vec());
    };
    if bytes[range.clone()] != limit.written {
        return Err(Error::at(
            "tool_output_token_limit",
            "the current value differs from the recorded value; left it in place",
        ));
    }
    let (range, replacement) = if let Some(before) = &limit.before {
        (range, before.clone())
    } else {
        let range = source.assignment(LIMIT)?;
        if let Some(assignment) = &limit.assignment {
            let end = range.start + assignment.len();
            if bytes.get(range.start..end) != Some(assignment.as_slice()) {
                return Err(Error::at(
                    "tool_output_token_limit",
                    "the inserted assignment was edited; left it in place",
                ));
            }
            (range.start..end, vec![])
        } else {
            // Legacy receipts own the setting but not any of its formatting.
            (range, vec![])
        }
    };
    let next = splice(bytes, vec![(range, replacement)])?;
    Source::parse(&next, location)?;
    Ok(next)
}
