//! The two-field header is searchable text; the document body is kept verbatim.

use super::MAX_DOCUMENT_BYTES;

pub struct Parsed<'a> {
    pub description: String,
    pub body: &'a str,
}

pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 240
        || !name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err("a history name uses 1 to 240 lowercase letters, digits or hyphens".into());
    }
    if matches!(name, "con" | "prn" | "aux" | "nul")
        || (name.len() == 4
            && (name.starts_with("com") || name.starts_with("lpt"))
            && name.as_bytes()[3].is_ascii_digit())
    {
        return Err("this name is reserved by Windows; choose a more specific topic name".into());
    }
    Ok(())
}

/// Parses the header without changing any bytes, including quotation and line
/// endings. The first colon separates a field; colons inside its value are text.
pub fn parse<'a>(name: &str, text: &'a str) -> Result<Parsed<'a>, String> {
    check_name(name)?;
    if text.len() > MAX_DOCUMENT_BYTES {
        return Err(format!(
            "a history document is limited to {MAX_DOCUMENT_BYTES} bytes; split it into smaller topics"
        ));
    }
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().ok_or("a history document starts with ---")?;
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return Err("a history document starts with ---".into());
    }
    let mut offset = first.len();
    let mut stated_name = None;
    let mut description = None;
    for raw in lines {
        offset += raw.len();
        let line = raw.trim_end_matches(['\r', '\n']);
        if line == "---" {
            if stated_name.as_deref() != Some(name) {
                return Err("the name in the header must equal the filename without .md".into());
            }
            return Ok(Parsed {
                description: description.ok_or("the header needs a one-line description")?,
                body: &text[offset..],
            });
        }
        let (key, value) = line.split_once(':').ok_or("the header contains only name: and description:")?;
        let target = match key {
            "name" => &mut stated_name,
            "description" => &mut description,
            _ => return Err(format!("unknown history header field {key:?}; use only name and description")),
        };
        let value = scalar(value.trim())?;
        if value.trim().is_empty() || value.chars().any(char::is_control) {
            return Err(format!("history {key} must be a nonempty single line"));
        }
        if target.replace(value).is_some() {
            return Err(format!("history header repeats {key}"));
        }
    }
    Err("the history header needs a closing ---".into())
}

fn scalar(value: &str) -> Result<String, String> {
    if value.starts_with('"') {
        serde_json::from_str::<String>(value).map_err(|error| format!("invalid quoted header value: {error}"))
    } else if let Some(value) = value.strip_prefix('\'') {
        value.strip_suffix('\'').map(|value| value.replace("''", "'")).ok_or_else(|| "unclosed header quote".into())
    } else {
        Ok(value.into())
    }
}
