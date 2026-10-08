//! Ranged edits: untouched settings bytes never pass through a serializer.

use std::collections::BTreeSet;
use std::ops::Range;

use jsonc_parser::ast::{Array, Object, ObjectProp, Value};
use jsonc_parser::common::Ranged;
use jsonc_parser::tokens::{Token, TokenAndRange};
use jsonc_parser::{CollectOptions, CommentCollectionStrategy, ParseOptions, parse_to_ast};
use serde::{Deserialize, Serialize};

pub(super) struct Document<'a> {
    pub source: &'a str,
    bom: bool,
    pub root: Option<Object<'a>>,
    tokens: Vec<TokenAndRange<'a>>,
}

impl<'a> Document<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        let source = std::str::from_utf8(bytes).map_err(|e| format!("Claude settings are not UTF-8: {e}"))?;
        let bom = source.starts_with('\u{feff}');
        let source = source.strip_prefix('\u{feff}').unwrap_or(source);
        let parsed = parse_to_ast(
            source,
            &CollectOptions { comments: CommentCollectionStrategy::AsTokens, tokens: true },
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
        .map_err(|e| format!("Invalid Claude settings JSONC: {e}"))?;
        let root = match parsed.value {
            Some(Value::Object(object)) => {
                validate(&object)?;
                Some(object)
            }
            Some(_) => return Err("Claude settings must be a JSON object".into()),
            None => None,
        };
        Ok(Self { source, bom, root, tokens: parsed.tokens.unwrap_or_default() })
    }

    pub fn raw(&self, value: &Value<'_>) -> &str {
        let range = value.range();
        &self.source[range.start..range.end]
    }

    pub fn replace(&self, range: Range<usize>, text: &str) -> Result<Vec<u8>, String> {
        let mut result = self.source.to_owned();
        result.replace_range(range, text);
        Self::validate_bytes(result.as_bytes())?;
        Ok(self.encode(result))
    }

    fn encode(&self, text: String) -> Vec<u8> {
        if self.bom { format!("\u{feff}{text}").into_bytes() } else { text.into_bytes() }
    }

    fn validate_bytes(bytes: &[u8]) -> Result<(), String> {
        Document::parse(bytes).map(|_| ())
    }

    pub fn insert(&self, object: &Object<'_>, name: &str, raw: &str) -> Result<(Vec<u8>, Insertion), String> {
        let multiline = self.source[object.range.start..object.range.end].contains('\n');
        let prefix = if multiline {
            let indent = object
                .properties
                .first()
                .map(|p| self.indent(p.range.start))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("{}  ", self.indent(object.range.start)));
            format!("{}{indent}", if self.source.contains("\r\n") { "\r\n" } else { "\n" })
        } else {
            String::new()
        };
        let suffix = if object.properties.is_empty() { "" } else { "," }.to_owned();
        let property = format!("{}: {raw}", serde_json::to_string(name).expect("a key serializes"));
        let insertion = Insertion { prefix, suffix };
        let text = format!("{}{property}{}", insertion.prefix, insertion.suffix);
        let position = object.range.start + 1;
        Ok((self.replace(position..position, &text)?, insertion))
    }

    fn indent(&self, position: usize) -> String {
        let start = self.source[..position].rfind('\n').map_or(0, |i| i + 1);
        self.source[start..position].chars().take_while(|c| matches!(c, ' ' | '\t')).collect()
    }

    pub fn has_comments(&self, range: Range<usize>) -> bool {
        self.tokens.iter().any(|t| t.range.start >= range.start && t.range.end <= range.end && is_comment(t))
    }

    pub fn remove_element(&self, array: &Array<'_>, index: usize, prefix: &str) -> Result<Vec<u8>, String> {
        let value = array.elements[index].range();
        if let Some(start) = value.start.checked_sub(prefix.len())
            && self.source.get(start..value.start) == Some(prefix)
        {
            let mut next = self.source.to_owned();
            next.replace_range(start..value.end, "");
            if Self::validate_bytes(next.as_bytes()).is_ok() {
                return Ok(self.encode(next));
            }
        }
        let next = array.elements.get(index + 1).map_or(array.range.end - 1, |item| item.range().start);
        let previous = index.checked_sub(1).map(|i| array.elements[i].range().end);
        let comma = self
            .tokens
            .iter()
            .find(|token| {
                matches!(token.token, Token::Comma) && token.range.start >= value.end && token.range.end <= next
            })
            .or_else(|| {
                previous.and_then(|previous| {
                    self.tokens.iter().find(|token| {
                        matches!(token.token, Token::Comma)
                            && token.range.start >= previous
                            && token.range.end <= value.start
                    })
                })
            });
        let mut next = self.source.to_owned();
        if let Some(comma) = comma.filter(|token| token.range.start >= value.end) {
            next.replace_range(comma.range.start..comma.range.end, "");
        }
        next.replace_range(value.start..value.end, "");
        if let Some(comma) = comma.filter(|token| token.range.end <= value.start) {
            next.replace_range(comma.range.start..comma.range.end, "");
        }
        Self::validate_bytes(next.as_bytes())?;
        Ok(self.encode(next))
    }

    pub(super) fn comments(&self) -> Vec<String> {
        self.tokens
            .iter()
            .filter(|token| is_comment(token))
            .map(|token| self.source[token.range.start..token.range.end].to_owned())
            .collect()
    }

    pub fn remove(
        &self,
        object: &Object<'_>,
        property: &ObjectProp<'_>,
        insertion: Option<&Insertion>,
    ) -> Result<Vec<u8>, String> {
        let start = property.range.start;
        let end = property.range.end;
        if let Some(insertion) = insertion
            && let Some(before) = start.checked_sub(insertion.prefix.len())
            && self.source.get(before..start) == Some(&insertion.prefix)
            && self.source.get(end..end + insertion.suffix.len()) == Some(&insertion.suffix)
            && !self.has_comments(start..end)
        {
            // Exact inverse when our surrounding punctuation still exists. If
            // another property has been added, remove structurally below.
            let mut result = self.source.to_owned();
            result.replace_range(before..end + insertion.suffix.len(), "");
            if Self::validate_bytes(result.as_bytes()).is_ok() {
                return Ok(self.encode(result));
            }
        }
        let index =
            object.properties.iter().position(|p| p.range == property.range).expect("property belongs to object");
        let next = object.properties.get(index + 1).map_or(object.range.end - 1, |p| p.range.start);
        let previous = index.checked_sub(1).map(|i| object.properties[i].range.end);
        let comma = self
            .tokens
            .iter()
            .find(|t| matches!(t.token, Token::Comma) && t.range.start >= end && t.range.end <= next)
            .or_else(|| {
                previous.and_then(|previous| {
                    self.tokens
                        .iter()
                        .find(|t| matches!(t.token, Token::Comma) && t.range.start >= previous && t.range.end <= start)
                })
            });
        let mut result = self.source.to_owned();
        // Comments inserted inside the key/value pair belong to the user too.
        let mut trivia = self.source[start..end].to_owned();
        for token in self.tokens.iter().rev().filter(|t| t.range.start >= start && t.range.end <= end && !is_comment(t))
        {
            trivia.replace_range(token.range.start - start..token.range.end - start, "");
        }
        if let Some(comma) = comma.filter(|c| c.range.start >= end) {
            result.replace_range(comma.range.start..comma.range.end, "");
        }
        result.replace_range(start..end, &trivia);
        if let Some(comma) = comma.filter(|c| c.range.end <= start) {
            result.replace_range(comma.range.start..comma.range.end, "");
        }
        Self::validate_bytes(result.as_bytes())?;
        Ok(self.encode(result))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Insertion {
    prefix: String,
    suffix: String,
}

fn is_comment(token: &TokenAndRange<'_>) -> bool {
    matches!(token.token, Token::CommentLine(_) | Token::CommentBlock(_))
}

fn validate(object: &Object<'_>) -> Result<(), String> {
    let mut names = BTreeSet::new();
    for property in &object.properties {
        if !names.insert(property.name.as_str()) {
            return Err("Claude settings contain duplicate object keys".into());
        }
        validate_value(&property.value)?;
    }
    Ok(())
}

fn validate_value(value: &Value<'_>) -> Result<(), String> {
    match value {
        Value::Object(object) => validate(object),
        Value::Array(array) => {
            for value in &array.elements {
                validate_value(value)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
