//! Conservative source-preserving edits for TraeCode CLI's top-level YAML server list.

use serde_json::Value;
use yaml_rust2::{Yaml, YamlLoader};

use super::json_config::canonical_hash;

pub(super) struct TraeEntryEdit {
    pub bytes: Vec<u8>,
    pub managed_hash: String,
}

pub(super) fn apply(
    original: &[u8],
    desired: &Value,
    owned_hash: Option<&str>,
) -> Result<TraeEntryEdit, String> {
    let source = utf8(original)?;
    let parsed = parse(source)?;
    let layout = Layout::scan(source)?;
    let desired_hash = canonical_hash(desired);
    let newline = layout.newline;
    let mut output = source.to_string();

    match layout.server_key {
        None => {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push_str(newline);
            }
            output.push_str("mcp_servers:");
            output.push_str(newline);
            output.push_str(&render_entry(desired, "  ", newline)?);
        }
        Some(block) => {
            let servers = yaml_servers(&parsed)?;
            if servers.len() != block.items.len() {
                return Err(
                    "TraeCode mcp_servers uses a YAML shape that FastCtx cannot edit without reformatting; use a plain block sequence and retry."
                        .to_string(),
                );
            }
            let matches = servers
                .iter()
                .enumerate()
                .filter(|(_, value)| yaml_field(value, "name") == Some("fastctx"))
                .collect::<Vec<_>>();
            if matches.len() > 1 {
                return Err(
                    "TraeCode configuration contains multiple mcp_servers entries named fastctx."
                        .to_string(),
                );
            }
            if let Some((index, current)) = matches.first() {
                let current_hash = canonical_hash(&yaml_to_json(current)?);
                match owned_hash {
                    Some(expected) if current_hash == expected => {}
                    Some(_) => {
                        return Err(
                            "The managed TraeCode fastctx entry drifted after Apply; FastCtx will not overwrite user-changed bytes."
                                .to_string(),
                        );
                    }
                    None => {
                        return Err(
                            "TraeCode already has a fastctx server without a FastCtx ownership receipt. Rename or remove it, then retry Apply."
                                .to_string(),
                        );
                    }
                }
                let range = &block.items[*index];
                output.replace_range(
                    range.start..range.end,
                    &render_entry(desired, &range.indent, newline)?,
                );
            } else {
                let indent = block
                    .items
                    .first()
                    .map(|item| item.indent.as_str())
                    .unwrap_or("  ");
                output.insert_str(block.end, &render_entry(desired, indent, newline)?);
            }
        }
    }
    Ok(TraeEntryEdit {
        bytes: output.into_bytes(),
        managed_hash: desired_hash,
    })
}

pub(super) fn disconnect(original: &[u8], owned_hash: &str) -> Result<Vec<u8>, String> {
    let source = utf8(original)?;
    let parsed = parse(source)?;
    let layout = Layout::scan(source)?;
    let block = layout.server_key.ok_or_else(|| {
        "The managed TraeCode mcp_servers list is missing; FastCtx will not use stale ownership evidence."
            .to_string()
    })?;
    let servers = yaml_servers(&parsed)?;
    if servers.len() != block.items.len() {
        return Err(
            "TraeCode mcp_servers is not a plain block sequence; FastCtx will not delete from it."
                .to_string(),
        );
    }
    let matches = servers
        .iter()
        .enumerate()
        .filter(|(_, value)| yaml_field(value, "name") == Some("fastctx"))
        .collect::<Vec<_>>();
    let [(index, current)] = matches.as_slice() else {
        return Err(if matches.is_empty() {
            "The managed TraeCode fastctx entry is missing; FastCtx will not use stale ownership evidence."
                .to_string()
        } else {
            "TraeCode configuration contains multiple mcp_servers entries named fastctx."
                .to_string()
        });
    };
    if canonical_hash(&yaml_to_json(current)?) != owned_hash {
        return Err(
            "The managed TraeCode fastctx entry drifted after Apply; FastCtx will not delete user-changed bytes."
                .to_string(),
        );
    }
    let mut output = source.to_string();
    let range = &block.items[*index];
    output.replace_range(range.start..range.end, "");
    Ok(output.into_bytes())
}

pub(super) fn inspect(original: &[u8]) -> Result<Option<String>, String> {
    let source = utf8(original)?;
    let parsed = parse(source)?;
    let servers = yaml_servers(&parsed)?;
    let matches = servers
        .iter()
        .filter(|value| yaml_field(value, "name") == Some("fastctx"))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [value] => Ok(Some(canonical_hash(&yaml_to_json(value)?))),
        _ => Err(
            "TraeCode configuration contains multiple mcp_servers entries named fastctx."
                .to_string(),
        ),
    }
}

fn utf8(bytes: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(bytes)
        .map_err(|error| format!("TraeCode configuration is not valid UTF-8 ({error})."))
}

fn parse(source: &str) -> Result<Yaml, String> {
    if source.trim().is_empty() {
        return Ok(Yaml::Hash(Default::default()));
    }
    let documents = YamlLoader::load_from_str(source)
        .map_err(|error| format!("Cannot parse TraeCode YAML configuration: {error}"))?;
    if documents.len() != 1 {
        return Err("TraeCode configuration must contain exactly one YAML document.".to_string());
    }
    let root = documents.into_iter().next().unwrap();
    if contains_alias(&root) {
        return Err(
            "TraeCode configuration uses YAML aliases; FastCtx cannot edit it without changing semantics."
                .to_string(),
        );
    }
    if !matches!(root, Yaml::Hash(_)) {
        return Err("TraeCode configuration root must be a mapping.".to_string());
    }
    Ok(root)
}

fn yaml_servers(root: &Yaml) -> Result<&[Yaml], String> {
    let value = &root["mcp_servers"];
    match value {
        Yaml::BadValue | Yaml::Null => Ok(&[]),
        Yaml::Array(values) => Ok(values),
        _ => Err("TraeCode mcp_servers must be a block sequence.".to_string()),
    }
}

fn yaml_field<'a>(value: &'a Yaml, key: &str) -> Option<&'a str> {
    value[key].as_str()
}

fn contains_alias(value: &Yaml) -> bool {
    match value {
        Yaml::Alias(_) => true,
        Yaml::Array(values) => values.iter().any(contains_alias),
        Yaml::Hash(values) => values
            .iter()
            .any(|(key, value)| contains_alias(key) || contains_alias(value)),
        _ => false,
    }
}

fn yaml_to_json(value: &Yaml) -> Result<Value, String> {
    Ok(match value {
        Yaml::Null | Yaml::BadValue => Value::Null,
        Yaml::Boolean(value) => Value::Bool(*value),
        Yaml::Integer(value) => Value::Number((*value).into()),
        Yaml::Real(value) => Value::String(value.clone()),
        Yaml::String(value) => Value::String(value.clone()),
        Yaml::Array(values) => Value::Array(
            values
                .iter()
                .map(yaml_to_json)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Yaml::Hash(values) => {
            let mut output = serde_json::Map::new();
            for (key, value) in values {
                let Some(key) = key.as_str() else {
                    return Err("TraeCode managed server keys must be strings.".to_string());
                };
                output.insert(key.to_string(), yaml_to_json(value)?);
            }
            Value::Object(output)
        }
        Yaml::Alias(_) => {
            return Err("TraeCode managed server entries cannot contain aliases.".to_string());
        }
    })
}

fn render_entry(value: &Value, indent: &str, newline: &str) -> Result<String, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "The TraeCode managed entry must be an object.".to_string())?;
    let mut output = String::new();
    let mut first = true;
    for key in ["name", "type", "command", "args", "env"] {
        let Some(value) = object.get(key) else {
            continue;
        };
        if first {
            output.push_str(indent);
            output.push_str("- ");
            first = false;
        } else {
            output.push_str(indent);
            output.push_str("  ");
        }
        output.push_str(key);
        match value {
            Value::String(value) => {
                output.push_str(": ");
                output.push_str(&serde_json::to_string(value).unwrap());
                output.push_str(newline);
            }
            Value::Array(values) => {
                output.push(':');
                output.push_str(newline);
                for value in values {
                    let value = value
                        .as_str()
                        .ok_or_else(|| format!("TraeCode managed {key} values must be strings."))?;
                    output.push_str(indent);
                    output.push_str("    - ");
                    output.push_str(&serde_json::to_string(value).unwrap());
                    output.push_str(newline);
                }
            }
            Value::Object(values) => {
                output.push(':');
                output.push_str(newline);
                for (name, value) in values {
                    let value = value
                        .as_str()
                        .ok_or_else(|| format!("TraeCode managed {key} values must be strings."))?;
                    output.push_str(indent);
                    output.push_str("    ");
                    output.push_str(&serde_json::to_string(name).unwrap());
                    output.push_str(": ");
                    output.push_str(&serde_json::to_string(value).unwrap());
                    output.push_str(newline);
                }
            }
            _ => return Err(format!("Unsupported TraeCode managed field {key}.")),
        }
    }
    Ok(output)
}

struct Layout<'a> {
    newline: &'a str,
    server_key: Option<ServerBlock>,
}

struct ServerBlock {
    end: usize,
    items: Vec<ItemRange>,
}

struct ItemRange {
    start: usize,
    end: usize,
    indent: String,
}

impl<'a> Layout<'a> {
    fn scan(source: &'a str) -> Result<Self, String> {
        let newline = if source.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let mut lines = Vec::new();
        let mut offset = 0;
        for line in source.split_inclusive('\n') {
            let end = offset + line.len();
            lines.push((offset, end, line.trim_end_matches(['\r', '\n'])));
            offset = end;
        }
        if offset < source.len() || source.is_empty() {
            lines.push((offset, source.len(), &source[offset..]));
        }
        let keys = lines
            .iter()
            .enumerate()
            .filter(|(_, (_, _, line))| {
                !line.starts_with(char::is_whitespace)
                    && line
                        .split('#')
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .starts_with("mcp_servers:")
            })
            .collect::<Vec<_>>();
        if keys.len() > 1 {
            return Err(
                "TraeCode configuration contains duplicate top-level mcp_servers keys.".to_string(),
            );
        }
        let Some((key_index, (_, _, key_line))) = keys.first().copied() else {
            return Ok(Self {
                newline,
                server_key: None,
            });
        };
        let key_value = key_line
            .split('#')
            .next()
            .unwrap_or_default()
            .trim()
            .strip_prefix("mcp_servers:")
            .unwrap()
            .trim();
        if !key_value.is_empty() {
            return Err(
                "TraeCode mcp_servers must use a plain block sequence, not an inline value."
                    .to_string(),
            );
        }
        let mut block_end = source.len();
        for (_, (start, _, line)) in lines.iter().enumerate().skip(key_index + 1) {
            let content = line.split('#').next().unwrap_or_default();
            if !content.trim().is_empty() && !content.starts_with(char::is_whitespace) {
                block_end = *start;
                break;
            }
        }
        let mut starts = Vec::new();
        let mut item_indent = None;
        for (start, _, line) in lines.iter().skip(key_index + 1) {
            if *start >= block_end {
                break;
            }
            let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
            let trimmed = &line[indent_len..];
            if indent_len > 0 && (trimmed == "-" || trimmed.starts_with("- ")) {
                match item_indent {
                    None => {
                        item_indent = Some(indent_len);
                        starts.push((*start, line[..indent_len].to_string()));
                    }
                    Some(expected) if indent_len == expected => {
                        starts.push((*start, line[..indent_len].to_string()));
                    }
                    Some(_) => {}
                }
            }
        }
        let items = starts
            .iter()
            .enumerate()
            .map(|(index, (start, indent))| ItemRange {
                start: *start,
                end: starts
                    .get(index + 1)
                    .map(|(next, _)| *next)
                    .unwrap_or(block_end),
                indent: indent.clone(),
            })
            .collect();
        Ok(Self {
            newline,
            server_key: Some(ServerBlock {
                end: block_end,
                items,
            }),
        })
    }
}
