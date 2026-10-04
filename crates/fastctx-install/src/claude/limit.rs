//! Claude Code 2.1.287 coerces settings.env values with JavaScript String,
//! then parses this variable with its integer environment parser (`Ic`).
//! Its scientific/grouped-number fast path is limited to 32 UTF-16 units;
//! otherwise it consumes a decimal prefix. Nonpositive/invalid means 25,000.

use std::sync::LazyLock;

use jsonc_parser::ast::Value;
use regex_lite::Regex;

pub(super) fn effective(value: &Value<'_>) -> Result<f64, String> {
    let text = js_string(value)?;
    let text = text.trim_matches(js_whitespace);
    let number = parse(text);
    Ok(if number.is_finite() && number > 0.0 { number } else { 25_000.0 })
}

fn parse(text: &str) -> f64 {
    static SCIENTIFIC: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[+-]?([0-9]+(\.[0-9]*)?|\.[0-9]+)[eE][+-]?[0-9]+$").expect("literal regex"));
    if text.encode_utf16().count() <= 32 {
        if SCIENTIFIC.is_match(text) {
            let number = text.parse::<f64>().unwrap_or(f64::NAN);
            return if number.fract() == 0.0 { number } else { f64::NAN };
        }
        let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
        if let Some(separator) = unsigned.chars().find(|c| matches!(c, '_' | ',' | '\u{a0}' | '\u{202f}' | ' ')) {
            let groups: Vec<_> = unsigned.split(separator).collect();
            if (1..=3).contains(&groups[0].len())
                && groups.iter().all(|g| g.bytes().all(|c| c.is_ascii_digit()))
                && groups[1..].iter().all(|g| g.len() == 3)
            {
                return text.replace(separator, "").parse::<f64>().unwrap_or(f64::NAN);
            }
        }
    }
    let sign = usize::from(text.starts_with(['+', '-']));
    let digits = text[sign..].bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 { f64::NAN } else { text[..sign + digits].parse::<f64>().unwrap_or(f64::NAN) }
}

fn js_string(value: &Value<'_>) -> Result<String, String> {
    Ok(match value {
        Value::StringLit(s) => s.value.to_string(),
        Value::NumberLit(n) => {
            let number = n.value.parse::<f64>().map_err(|_| "Cannot interpret Claude's numeric output limit")?;
            ryu_js::Buffer::new().format(number).to_owned()
        }
        Value::BooleanLit(b) => b.value.to_string(),
        Value::NullKeyword(_) => "null".into(),
        Value::Array(a) => a
            .elements
            .iter()
            .map(|v| if matches!(v, Value::NullKeyword(_)) { Ok(String::new()) } else { js_string(v) })
            .collect::<Result<Vec<_>, _>>()?
            .join(","),
        Value::Object(o) => {
            if o.get("toString").is_some() {
                return Err("Claude cannot coerce this env value to a string".into());
            }
            "[object Object]".into()
        }
    })
}

fn js_whitespace(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}
