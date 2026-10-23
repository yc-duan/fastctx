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
        return Err("a memory name must contain 1 to 240 UTF-8 bytes".into());
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
            return Err("a memory name uses letters, numbers and hyphens; combining marks follow a letter".into());
        }
    }
    let upper = name.to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.as_bytes()[3].is_ascii_digit())
    {
        return Err("this memory name is reserved by Windows".into());
    }
    Ok(())
}

/// Reads the required one-line fields and ignores only World-owned attribution.
pub fn parse<'a>(name: &str, text: &'a str) -> Result<Parsed<'a>, String> {
    check_name(name)?;
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().ok_or("memory Markdown must start with ---")?;
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return Err("memory Markdown must start with ---".into());
    }
    let mut offset = first.len();
    let mut stated_name = None;
    let mut description = None;
    for raw in lines {
        offset += raw.len();
        let line = raw.trim_end_matches(['\r', '\n']);
        if line == "---" {
            if stated_name.as_deref() != Some(name) {
                return Err("the header's name must match the Markdown filename".into());
            }
            let description = description.ok_or("the header needs a nonempty one-line description")?;
            return Ok(Parsed { description, body: &text[offset..] });
        }
        let (key, value) = line.split_once(':').ok_or("each header field must be key: value on one line")?;
        match key {
            "name" | "description" => {
                let value = scalar(value.trim())?;
                if value.trim().is_empty() || value.chars().any(char::is_control) {
                    return Err(format!("{key} must be nonempty and on one line"));
                }
                let field = if key == "name" { &mut stated_name } else { &mut description };
                if field.replace(value).is_some() {
                    return Err(format!("the header repeats {key}"));
                }
            }
            "written_at" | "conversation" | "host" | "model" | "machine" => {}
            _ => return Err(format!("unknown memory header field {key:?}")),
        }
    }
    Err("the memory header needs a closing ---".into())
}

fn scalar(value: &str) -> Result<String, String> {
    if value.starts_with('"') {
        return serde_json::from_str::<String>(value).map_err(|error| format!("invalid quoted header field: {error}"));
    }
    if let Some(value) = value.strip_prefix('\'') {
        return value
            .strip_suffix('\'')
            .map(|value| value.replace("''", "'"))
            .ok_or_else(|| "a quoted header field needs its closing quote".into());
    }
    Ok(value.to_string())
}

/// Replaces caller-supplied attribution with the facts supplied by the World.
pub fn render(name: &str, kind: Kind, text: &str, author: &Author) -> Result<(String, String), String> {
    let parsed = parse(name, text)?;
    let quote = |value: &str| serde_json::to_string(value).expect("strings serialize");
    let mut result = format!("---\nname: {name}\ndescription: {}\n", quote(&parsed.description));
    if kind == Kind::Journal {
        result.push_str(&format!("written_at: {}\nmachine: {}\n", author.stamp.at, quote(author.stamp.on.as_str())));
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
