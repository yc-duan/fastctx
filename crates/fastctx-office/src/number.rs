//! Render saved numbers without evaluating formulas or applying display rounding.
use crate::output::quoted;
use office_oxide::xlsx::DateTimeValue;

pub(crate) fn saved(raw: &str, value: f64, code: Option<&str>, id: Option<u32>, date1904: bool) -> String {
    if code.is_some() {
        return format(raw, value, code, date1904);
    }
    let Some(id) = id.filter(|&id| id != 0) else { return raw.into() };
    // ECMA-376 18.8.30 reserves these localized built-ins. Their calendar
    // values are locale-independent even when their display codes are not.
    let canonical = match id {
        27..=31 | 36 | 50 | 51 | 54 | 57 | 58 | 71..=74 | 81 => Some("yyyy-mm-dd"),
        32 | 33 | 75 | 76 | 78 => Some("hh:mm:ss"),
        80 => Some("hh:mm:ss.0"),
        77 => Some("yyyy-mm-dd hh:mm:ss"),
        34 | 35 | 52 | 53 | 55 | 56 if value >= 1.0 || date1904 => Some("yyyy-mm-dd hh:mm:ss"),
        _ => None,
    };
    if let Some(pattern) = canonical {
        let text = format(raw, value, Some(pattern), date1904);
        if !text.contains(" (format ") {
            return text;
        }
        return format!("{raw} (format id {id}; date could not be converted)");
    }
    format!("{raw} (format id {id})")
}

pub(crate) fn format(raw: &str, number: f64, code: Option<&str>, date1904: bool) -> String {
    let Some(code) = code.filter(|s| !s.eq_ignore_ascii_case("General")) else { return raw.into() };
    let active = section(code, number);
    let (date, time, elapsed, precision) = active.map(classify).unwrap_or_default();
    if (date || time) && !elapsed {
        if !number.is_finite() || number < 0.0 {
            return format!("{raw} (format {}; date could not be converted)", quoted(code));
        }
        let Some(scale) = 10u64.checked_pow(precision) else {
            return format!("{raw} (format {}; date precision could not be converted)", quoted(code));
        };
        let Some(day_ticks) = 86400u64.checked_mul(scale) else {
            return format!("{raw} (format {}; date precision could not be converted)", quoted(code));
        };
        let ticks = (number.fract() * day_ticks as f64).round();
        let day = number.trunc() + if ticks >= day_ticks as f64 { 1.0 } else { 0.0 };
        let ticks = if ticks >= day_ticks as f64 { 0 } else { ticks as u64 };
        let clock = ticks / scale;
        let mut clock_text = format!("{:02}:{:02}:{:02}", clock / 3600, clock / 60 % 60, clock % 60);
        if precision > 0 {
            clock_text.push_str(&format!(".{:0width$}", ticks % scale, width = precision as usize));
        }
        if !date && (0.0..1.0).contains(&number) {
            return clock_text;
        }
        let date_text = if !date1904 && day == 60.0 {
            Some("1900-02-29".to_owned())
        } else {
            DateTimeValue::from_serial(day, date1904).map(|d| format!("{:04}-{:02}-{:02}", d.year, d.month, d.day))
        };
        if let Some(date_text) = date_text {
            return if time || clock > 0 { format!("{date_text} {clock_text}") } else { date_text };
        }
        return format!("{raw} (format {}; date could not be converted)", quoted(code));
    }
    format!("{raw} (format {})", quoted(code))
}

fn section(code: &str, value: f64) -> Option<&str> {
    let mut parts = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (at, c) in code.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if !quoted => escaped = true,
            '"' => quoted = !quoted,
            ';' if !quoted => {
                parts.push(&code[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    parts.push(&code[start..]);
    let numeric = &parts[..parts.len().min(3)];
    let conditions: Vec<_> = numeric.iter().map(|part| condition(part)).collect();
    if conditions.iter().any(Option::is_some) {
        for (part, condition) in numeric.iter().zip(conditions) {
            let Some((op, threshold)) = condition else { return Some(part) };
            let matches = match op {
                "<" => value < threshold,
                "<=" => value <= threshold,
                ">" => value > threshold,
                ">=" => value >= threshold,
                "=" => value == threshold,
                "<>" => value != threshold,
                _ => unreachable!(),
            };
            if matches {
                return Some(part);
            }
        }
        return None;
    }
    Some(match numeric.len() {
        1 => numeric[0],
        2 => numeric[usize::from(value < 0.0)],
        _ => {
            numeric[if value > 0.0 {
                0
            } else if value < 0.0 {
                1
            } else {
                2
            }]
        }
    })
}

fn condition(mut code: &str) -> Option<(&str, f64)> {
    while let Some(tail) = code.strip_prefix('[') {
        let (bracket, rest) = tail.split_once(']')?;
        for op in ["<=", ">=", "<>", "<", ">", "="] {
            if let Some(number) = bracket.strip_prefix(op) {
                return number.parse().ok().map(|value| (op, value));
            }
        }
        code = rest;
    }
    None
}
fn classify(code: &str) -> (bool, bool, bool, u32) {
    let mut chars = code.chars();
    let mut tokens = String::new();
    let mut elapsed = false;
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                for c in chars.by_ref() {
                    if c == '"' {
                        break;
                    }
                }
            }
            '\\' | '_' | '*' => {
                chars.next();
            }
            '[' => {
                let mut part = String::new();
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                    part.push(c.to_ascii_lowercase());
                }
                if matches!(part.as_str(), "h" | "hh" | "m" | "mm" | "s" | "ss") {
                    elapsed = true;
                }
            }
            ';' => break,
            _ => tokens.push(c.to_ascii_lowercase()),
        }
    }
    let time = tokens.contains(['h', 's']);
    let date = tokens.contains(['y', 'd']) || (tokens.contains('m') && !time);
    let precision = tokens.find("s.").map_or(0, |at| tokens[at + 2..].chars().take_while(|&c| c == '0').count() as u32);
    (date, time, elapsed, precision)
}
