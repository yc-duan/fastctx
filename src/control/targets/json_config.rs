//! Byte-preserving JSONC edits over a validated ranged syntax tree.

use crate::control::settings::JsoncConfigReceipt;
use jsonc_parser::ast::{Object, ObjectProp, Value as AstValue};
use jsonc_parser::common::{Range, Ranged};
use jsonc_parser::tokens::{Token, TokenAndRange};
use jsonc_parser::{CollectOptions, CommentCollectionStrategy, ParseOptions, parse_to_ast};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(super) struct JsonEntryEdit {
    pub bytes: Vec<u8>,
    pub managed_hash: String,
    pub receipt: JsoncConfigReceipt,
}

pub(super) fn apply(
    original: &[u8],
    property_path: &[&str],
    desired: &Value,
    owned_hash: Option<&str>,
    previous_receipt: Option<&JsoncConfigReceipt>,
) -> Result<JsonEntryEdit, String> {
    if property_path.is_empty() {
        return Err("The managed JSONC property path cannot be empty.".to_string());
    }
    let source = utf8(original)?;
    let parsed = parse(source)?;
    let desired_hash = canonical_hash(desired);
    let Some(root) = parsed.root.as_ref() else {
        let value = nested_value(property_path, desired);
        let mut output = serde_json::to_string_pretty(&value)
            .map_err(|error| format!("Cannot render agent JSONC configuration: {error}"))?;
        output.push('\n');
        return Ok(JsonEntryEdit {
            bytes: output.into_bytes(),
            managed_hash: desired_hash,
            receipt: JsoncConfigReceipt {
                inserted_path: vec![property_path[0].to_string()],
                parent_had_trailing_comma: false,
                root_was_empty: true,
            },
        });
    };

    let mut parent = root;
    let mut missing = None;
    for (index, segment) in property_path[..property_path.len() - 1].iter().enumerate() {
        match parent.get(segment) {
            Some(property) => {
                parent = property.value.as_object().ok_or_else(|| {
                    format!(
                        "Cannot manage JSONC path {} because \"{segment}\" is not an object.",
                        property_path.join(".")
                    )
                })?;
            }
            None => {
                missing = Some(index);
                break;
            }
        }
    }

    let leaf_index = property_path.len() - 1;
    if missing.is_none() {
        let leaf = property_path[leaf_index];
        if let Some(property) = parent.get(leaf) {
            let current_hash = observed_hash(source, &parsed.tokens, property);
            match owned_hash {
                Some(expected) if current_hash == expected => {}
                Some(_) => {
                    return Err(format!(
                        "Managed JSONC entry {} drifted after Apply; FastCtx will not overwrite user-changed bytes.",
                        property_path.join(".")
                    ));
                }
                None => {
                    return Err(format!(
                        "JSONC entry {} already exists without a FastCtx ownership receipt. Rename or remove it, then retry Apply.",
                        property_path.join(".")
                    ));
                }
            }
            let pretty = object_uses_own_closing_line(source, parent);
            let indent = line_indent(source, property.range.start);
            let replacement = render_value(desired, &indent, pretty)?;
            let mut output = source.to_string();
            output.replace_range(
                property.value.range().start..property.value.range().end,
                &replacement,
            );
            parse(&output)?;
            return Ok(JsonEntryEdit {
                bytes: output.into_bytes(),
                managed_hash: desired_hash,
                receipt: previous_receipt.cloned().unwrap_or_default(),
            });
        }
        missing = Some(leaf_index);
    }

    let insertion_index = missing.expect("a missing JSONC path segment was selected");
    let insertion_name = property_path[insertion_index];
    let insertion_value = nested_value(&property_path[insertion_index + 1..], desired);
    let (output, parent_had_trailing_comma) = append_property(
        source,
        &parsed.tokens,
        parent,
        insertion_name,
        &insertion_value,
    )?;
    parse(&output)?;
    Ok(JsonEntryEdit {
        bytes: output.into_bytes(),
        managed_hash: desired_hash,
        receipt: JsoncConfigReceipt {
            inserted_path: property_path[..=insertion_index]
                .iter()
                .map(|segment| (*segment).to_string())
                .collect(),
            parent_had_trailing_comma,
            root_was_empty: false,
        },
    })
}

pub(super) fn disconnect(
    original: &[u8],
    property_path: &[&str],
    owned_hash: &str,
    receipt: Option<&JsoncConfigReceipt>,
) -> Result<Vec<u8>, String> {
    if property_path.is_empty() {
        return Err("The managed JSONC property path cannot be empty.".to_string());
    }
    let source = utf8(original)?;
    let parsed = parse(source)?;
    let root = parsed.root.as_ref().ok_or_else(|| {
        format!(
            "Managed JSONC entry {} is missing; FastCtx will not use stale ownership evidence.",
            property_path.join(".")
        )
    })?;
    let leaf = locate_property(root, property_path)?.ok_or_else(|| {
        format!(
            "Managed JSONC entry {} is missing; FastCtx will not use stale ownership evidence.",
            property_path.join(".")
        )
    })?;
    if observed_hash(source, &parsed.tokens, leaf.property) != owned_hash {
        return Err(format!(
            "Managed JSONC entry {} drifted after Apply; FastCtx will not delete user-changed bytes.",
            property_path.join(".")
        ));
    }

    let fallback = JsoncConfigReceipt::default();
    let receipt = receipt.unwrap_or(&fallback);
    if receipt.root_was_empty
        && root.properties.len() == 1
        && subtree_contains_only_path(root, &receipt.inserted_path, property_path)?
        && !parsed.tokens.iter().any(is_comment)
    {
        return Ok(Vec::new());
    }

    let inserted_path = receipt
        .inserted_path
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let can_remove_inserted_subtree = !inserted_path.is_empty()
        && property_path.starts_with(&inserted_path)
        && subtree_contains_only_path(root, &receipt.inserted_path, property_path)?
        && locate_property(root, &inserted_path)?
            .is_some_and(|located| !has_comment(&parsed.tokens, located.property.range));
    let removal_path = if can_remove_inserted_subtree {
        inserted_path.as_slice()
    } else {
        property_path
    };
    let exact_insert = can_remove_inserted_subtree;
    let mut output = remove_property(
        source,
        &parsed,
        removal_path,
        exact_insert.then_some(receipt.parent_had_trailing_comma),
    )?;
    parse(&output)?;
    if inspect(output.as_bytes(), property_path)?.is_some() {
        return Err(format!(
            "Managed JSONC entry {} remained after Disconnect planning.",
            property_path.join(".")
        ));
    }
    if output.is_empty() {
        return Ok(Vec::new());
    }
    Ok(std::mem::take(&mut output).into_bytes())
}

pub(super) fn inspect(original: &[u8], property_path: &[&str]) -> Result<Option<String>, String> {
    let source = utf8(original)?;
    let parsed = parse(source)?;
    let Some(root) = parsed.root.as_ref() else {
        return Ok(None);
    };
    Ok(locate_property(root, property_path)?
        .map(|located| observed_hash(source, &parsed.tokens, located.property)))
}

pub(super) fn canonical_hash(value: &Value) -> String {
    let sorted = sort_json(value);
    let bytes = serde_json::to_vec(&sorted).expect("JSON values are serializable");
    hex::encode(Sha256::digest(bytes))
}

struct ParsedDocument<'a> {
    root: Option<Object<'a>>,
    tokens: Vec<TokenAndRange<'a>>,
}

struct LocatedProperty<'document, 'source> {
    parent: &'document Object<'source>,
    property: &'document ObjectProp<'source>,
    index: usize,
}

fn utf8(bytes: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(bytes)
        .map_err(|error| format!("Agent JSONC configuration is not valid UTF-8 ({error})."))
}

fn parse(source: &str) -> Result<ParsedDocument<'_>, String> {
    let parsed = parse_to_ast(
        source,
        &CollectOptions {
            comments: CommentCollectionStrategy::AsTokens,
            tokens: true,
        },
        &ParseOptions {
            allow_comments: true,
            allow_trailing_commas: true,
            allow_loose_object_property_names: false,
            allow_missing_commas: false,
            allow_single_quoted_strings: false,
            allow_hexadecimal_numbers: false,
            allow_unary_plus_numbers: false,
        },
    )
    .map_err(|error| format!("Cannot parse agent JSONC configuration: {error}"))?;
    let root = match parsed.value {
        Some(AstValue::Object(object)) => {
            validate_object(&object, "root")?;
            Some(object)
        }
        Some(_) => {
            return Err("Agent JSONC configuration root must be an object.".to_string());
        }
        None => None,
    };
    Ok(ParsedDocument {
        root,
        tokens: parsed.tokens.unwrap_or_default(),
    })
}

fn validate_object(object: &Object<'_>, context: &str) -> Result<(), String> {
    let mut names = BTreeSet::new();
    for property in &object.properties {
        let name = property.name.as_str();
        if !names.insert(name.to_string()) {
            return Err(format!(
                "Agent JSONC configuration contains duplicate key \"{name}\" at {context}."
            ));
        }
        validate_value(&property.value, &format!("{context}.{name}"))?;
    }
    Ok(())
}

fn validate_value(value: &AstValue<'_>, context: &str) -> Result<(), String> {
    match value {
        AstValue::Object(object) => validate_object(object, context),
        AstValue::Array(array) => {
            for (index, value) in array.elements.iter().enumerate() {
                validate_value(value, &format!("{context}[{index}]"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn locate_property<'document, 'source>(
    root: &'document Object<'source>,
    path: &[&str],
) -> Result<Option<LocatedProperty<'document, 'source>>, String> {
    let Some((leaf, parents)) = path.split_last() else {
        return Ok(None);
    };
    let mut object = root;
    for segment in parents {
        let Some(property) = object.get(segment) else {
            return Ok(None);
        };
        object = property.value.as_object().ok_or_else(|| {
            format!(
                "Cannot inspect JSONC path {} because \"{segment}\" is not an object.",
                path.join(".")
            )
        })?;
    }
    Ok(object
        .properties
        .iter()
        .position(|property| property.name.as_str() == *leaf)
        .map(|index| LocatedProperty {
            parent: object,
            property: &object.properties[index],
            index,
        }))
}

fn nested_value(path: &[&str], leaf: &Value) -> Value {
    path.iter().rev().fold(leaf.clone(), |value, segment| {
        let mut object = serde_json::Map::new();
        object.insert((*segment).to_string(), value);
        Value::Object(object)
    })
}

fn append_property(
    source: &str,
    tokens: &[TokenAndRange<'_>],
    object: &Object<'_>,
    name: &str,
    value: &Value,
) -> Result<(String, bool), String> {
    let close = object
        .range
        .end
        .checked_sub(1)
        .ok_or_else(|| "Agent JSONC object has no closing brace.".to_string())?;
    let last = object.properties.last();
    let had_trailing_comma = last
        .and_then(|property| comma_between(tokens, property.range.end, close))
        .is_some();
    let own_closing_line = object_uses_own_closing_line(source, object);
    let property_indent = last.map_or_else(
        || format!("{}  ", line_indent(source, close)),
        |property| line_indent(source, property.range.start),
    );
    let rendered_value = render_value(value, &property_indent, own_closing_line)?;
    let name = serde_json::to_string(name)
        .map_err(|error| format!("Cannot render agent JSONC property name: {error}"))?;
    let mut property_text = format!("{name}: {rendered_value}");
    let mut output = source.to_string();
    if own_closing_line {
        if had_trailing_comma {
            property_text.push(',');
        }
        property_text = format!("{property_indent}{property_text}{}", newline(source));
        let insertion = line_start(source, close);
        output.insert_str(insertion, &property_text);
    } else {
        if !object.properties.is_empty() {
            property_text.insert(0, ' ');
        }
        if had_trailing_comma {
            property_text.push(',');
        }
        output.insert_str(close, &property_text);
    }
    if let Some(last) = last
        && !had_trailing_comma
    {
        output.insert(last.range.end, ',');
    }
    Ok((output, had_trailing_comma))
}

fn render_value(value: &Value, indent: &str, pretty: bool) -> Result<String, String> {
    let rendered = if pretty {
        serde_json::to_string_pretty(value)
    } else {
        serde_json::to_string(value)
    }
    .map_err(|error| format!("Cannot render agent JSONC value: {error}"))?;
    if pretty && rendered.contains('\n') {
        Ok(rendered.replace('\n', &format!("\n{indent}")))
    } else {
        Ok(rendered)
    }
}

fn remove_property(
    source: &str,
    parsed: &ParsedDocument<'_>,
    path: &[&str],
    original_parent_had_trailing_comma: Option<bool>,
) -> Result<String, String> {
    let root = parsed
        .root
        .as_ref()
        .ok_or_else(|| "Agent JSONC configuration root is missing.".to_string())?;
    let located = locate_property(root, path)?.ok_or_else(|| {
        format!(
            "Managed JSONC entry {} is missing; FastCtx will not use stale ownership evidence.",
            path.join(".")
        )
    })?;
    if let Some(had_trailing_comma) = original_parent_had_trailing_comma
        && located.index + 1 == located.parent.properties.len()
    {
        return remove_inserted_property(source, &parsed.tokens, located, had_trailing_comma);
    }
    remove_generic_property(source, &parsed.tokens, located)
}

fn remove_inserted_property(
    source: &str,
    tokens: &[TokenAndRange<'_>],
    located: LocatedProperty<'_, '_>,
    parent_had_trailing_comma: bool,
) -> Result<String, String> {
    let property = located.property;
    let close = located.parent.range.end.saturating_sub(1);
    let trailing = comma_between(tokens, property.range.end, close);
    let start_line = line_start(source, property.range.start);
    let own_line = source[start_line..property.range.start]
        .chars()
        .all(char::is_whitespace);
    let line_end = line_end(
        source,
        trailing.map_or(property.range.end, |range| range.end),
    );
    if own_line && !has_comment(tokens, Range::new(property.range.end, line_end)) {
        let mut output = source.to_string();
        output.replace_range(start_line..line_end, "");
        if !parent_had_trailing_comma
            && located.index > 0
            && let Some(comma) = comma_between(
                tokens,
                located.parent.properties[located.index - 1].range.end,
                property.range.start,
            )
        {
            output.replace_range(comma.start..comma.end, "");
        }
        return Ok(output);
    }

    let mut start = property.range.start;
    if located.index > 0 && source.as_bytes().get(start.wrapping_sub(1)) == Some(&b' ') {
        start -= 1;
    }
    let end = trailing.map_or(property.range.end, |range| range.end);
    let mut output = source.to_string();
    output.replace_range(start..end, "");
    if !parent_had_trailing_comma
        && located.index > 0
        && let Some(comma) = comma_between(
            tokens,
            located.parent.properties[located.index - 1].range.end,
            property.range.start,
        )
    {
        output.replace_range(comma.start..comma.end, "");
    }
    Ok(output)
}

fn remove_generic_property(
    source: &str,
    tokens: &[TokenAndRange<'_>],
    located: LocatedProperty<'_, '_>,
) -> Result<String, String> {
    let property = located.property;
    let next_start = located
        .parent
        .properties
        .get(located.index + 1)
        .map_or(located.parent.range.end.saturating_sub(1), |next| {
            next.range.start
        });
    let trailing = comma_between(tokens, property.range.end, next_start);
    let start_line = line_start(source, property.range.start);
    let own_line = source[start_line..property.range.start]
        .chars()
        .all(char::is_whitespace);
    let line_end = line_end(
        source,
        trailing.map_or(property.range.end, |range| range.end),
    );
    let mut output = source.to_string();
    let mut previous_comma = None;
    if located.index + 1 == located.parent.properties.len()
        && trailing.is_none()
        && located.index > 0
    {
        previous_comma = comma_between(
            tokens,
            located.parent.properties[located.index - 1].range.end,
            property.range.start,
        );
    }
    if own_line && !has_comment(tokens, Range::new(property.range.end, line_end)) {
        output.replace_range(start_line..line_end, "");
    } else {
        let end = trailing.map_or(property.range.end, |range| range.end);
        output.replace_range(property.range.start..end, "");
    }
    if let Some(comma) = previous_comma {
        output.replace_range(comma.start..comma.end, "");
    }
    Ok(output)
}

fn subtree_contains_only_path(
    root: &Object<'_>,
    inserted_path: &[String],
    full_path: &[&str],
) -> Result<bool, String> {
    if inserted_path.is_empty() || inserted_path.len() > full_path.len() {
        return Ok(false);
    }
    let inserted = inserted_path.iter().map(String::as_str).collect::<Vec<_>>();
    if !full_path.starts_with(&inserted) {
        return Ok(false);
    }
    let Some(located) = locate_property(root, &inserted)? else {
        return Ok(false);
    };
    let mut value = &located.property.value;
    for segment in &full_path[inserted.len()..] {
        let Some(object) = value.as_object() else {
            return Ok(false);
        };
        if object.properties.len() != 1 || object.properties[0].name.as_str() != *segment {
            return Ok(false);
        }
        value = &object.properties[0].value;
    }
    Ok(true)
}

fn observed_hash(source: &str, tokens: &[TokenAndRange<'_>], property: &ObjectProp<'_>) -> String {
    if has_comment(tokens, property.range) {
        let mut hasher = Sha256::new();
        hasher.update(b"commented-jsonc-entry\0");
        hasher.update(property.text(source).as_bytes());
        return hex::encode(hasher.finalize());
    }
    canonical_hash(&serde_json::Value::from(property.value.clone()))
}

fn object_uses_own_closing_line(source: &str, object: &Object<'_>) -> bool {
    let close = object.range.end.saturating_sub(1);
    source[object.range.start..object.range.end].contains('\n')
        && source[line_start(source, close)..close]
            .chars()
            .all(char::is_whitespace)
}

fn comma_between(tokens: &[TokenAndRange<'_>], start: usize, end: usize) -> Option<Range> {
    tokens
        .iter()
        .find(|token| {
            token.range.start >= start
                && token.range.end <= end
                && matches!(token.token, Token::Comma)
        })
        .map(|token| token.range)
}

fn has_comment(tokens: &[TokenAndRange<'_>], range: Range) -> bool {
    tokens.iter().any(|token| {
        token.range.start >= range.start && token.range.end <= range.end && is_comment(token)
    })
}

fn is_comment(token: &TokenAndRange<'_>) -> bool {
    matches!(token.token, Token::CommentLine(_) | Token::CommentBlock(_))
}

fn line_start(source: &str, index: usize) -> usize {
    source[..index]
        .rfind('\n')
        .map_or(0, |position| position + 1)
}

fn line_end(source: &str, index: usize) -> usize {
    source[index..]
        .find('\n')
        .map_or(source.len(), |position| index + position + 1)
}

fn line_indent(source: &str, index: usize) -> String {
    source[line_start(source, index)..index]
        .chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .collect()
}

fn newline(source: &str) -> &'static str {
    if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn sort_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(sort_json).collect()),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|key| (key.clone(), sort_json(&values[key])))
                    .collect(),
            )
        }
        value => value.clone(),
    }
}
