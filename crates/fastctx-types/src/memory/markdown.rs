//! The Markdown header shared by journals and specs.

use icu_properties::CodePointMapData;
use icu_properties::props::{GeneralCategory, GeneralCategoryGroup};

use super::{Author, Kind};

pub struct Parsed<'a> {
    pub description: String,
    pub body: &'a str,
}

/// Names are portable filenames, retaining their original language and case.
pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 240 {
        return Err(crate::memory::words::NAME_LENGTH.into());
    }
    let categories = CodePointMapData::<GeneralCategory>::new();
    let mut letter = false;
    for character in name.chars() {
        let category = categories.get(character);
        if GeneralCategoryGroup::Letter.contains(category) || GeneralCategoryGroup::Number.contains(category) {
            letter = true;
        } else if character == '-' {
            letter = false;
        } else if !(letter && GeneralCategoryGroup::Mark.contains(category)) {
            return Err(crate::memory::words::NAME_CHARACTERS.into());
        }
    }
    let upper = name.to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.as_bytes()[3].is_ascii_digit())
    {
        return Err(crate::memory::words::RESERVED_NAME.into());
    }
    Ok(())
}

/// Reads the required one-line fields and ignores only World-owned attribution.
pub fn parse<'a>(name: &str, text: &'a str) -> Result<Parsed<'a>, String> {
    check_name(name)?;
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().ok_or(crate::memory::words::HEADER_START)?;
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return Err(crate::memory::words::HEADER_START.into());
    }
    let mut offset = first.len();
    let mut stated_name = None;
    let mut description = None;
    for raw in lines {
        offset += raw.len();
        let line = raw.trim_end_matches(['\r', '\n']);
        if line == "---" {
            if stated_name.as_deref() != Some(name) {
                return Err(crate::memory::words::HEADER_NAME.into());
            }
            let description = description.ok_or(crate::memory::words::HEADER_DESCRIPTION)?;
            return Ok(Parsed { description, body: &text[offset..] });
        }
        let (key, value) = line.split_once(':').ok_or(crate::memory::words::HEADER_FIELD)?;
        match key {
            "name" | "description" => {
                let value = scalar(value.trim())?;
                if value.trim().is_empty() || value.chars().any(char::is_control) {
                    return Err(crate::memory::words::header_value(key));
                }
                let field = if key == "name" { &mut stated_name } else { &mut description };
                if field.replace(value).is_some() {
                    return Err(crate::memory::words::header_repeat(key));
                }
            }
            "written_at" | "conversation" | "host" | "model" | "machine" => {}
            _ => return Err(crate::memory::words::header_unknown(key)),
        }
    }
    Err(crate::memory::words::HEADER_END.into())
}

fn scalar(value: &str) -> Result<String, String> {
    if value.starts_with('"') {
        return serde_json::from_str::<String>(value).map_err(crate::memory::words::header_quoted);
    }
    if let Some(value) = value.strip_prefix('\'') {
        return value
            .strip_suffix('\'')
            .map(|value| value.replace("''", "'"))
            .ok_or_else(|| crate::memory::words::HEADER_QUOTE.into());
    }
    Ok(value.to_string())
}

/// `text`, a journal as `render` wrote it, its header naming the machine
/// and the conversation as `author` now tells them (`Author::for_reader`).
pub fn attributed(text: &str, author: &Author) -> String {
    let Some(rest) = text.strip_prefix("---\n") else { return text.to_string() };
    let Some(end) = rest.find("\n---\n") else { return text.to_string() };
    let (header, body) = rest.split_at(end);
    let header: Vec<String> = header
        .lines()
        .map(|line| {
            if line.starts_with("machine: ") {
                format!("machine: {}", serde_json::to_string(&author.stamp.on).expect("names serialize"))
            } else if line.starts_with("conversation: ") {
                format!("conversation: {}", serde_json::to_string(&author.stamp.by).expect("writers serialize"))
            } else {
                line.to_string()
            }
        })
        .collect();
    format!("---\n{}{body}", header.join("\n"))
}

/// Replaces caller-supplied attribution with the facts supplied by the World.
pub fn render(name: &str, kind: Kind, text: &str, author: &Author) -> Result<(String, String), String> {
    let parsed = parse(name, text)?;
    let quote = |value: &str| serde_json::to_string(value).expect("strings serialize");
    let mut result = format!("---\nname: {name}\ndescription: {}\n", quote(&parsed.description));
    if kind == Kind::Journal {
        result.push_str(&format!(
            "written_at: {}\nmachine: {}\n",
            author.stamp.at,
            serde_json::to_string(&author.stamp.on).expect("names serialize")
        ));
        result.push_str(&format!(
            "conversation: {}\nhost: {}\nmodel: {}\n",
            serde_json::to_string(&author.stamp.by).expect("writers serialize"),
            serde_json::to_string(&author.host).expect("hosts serialize"),
            serde_json::to_string(&author.model).expect("models serialize")
        ));
    }
    result.push_str("---\n");
    result.push_str(parsed.body);
    Ok((parsed.description, result))
}
