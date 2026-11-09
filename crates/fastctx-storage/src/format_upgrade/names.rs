//! Portable backup basenames exclude Windows devices even with an extension.

pub(super) fn valid(name: &str) -> bool {
    if name.is_empty()
        || name.len() > 192
        || !name.to_ascii_lowercase().ends_with(".sqlite3")
        || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || name.starts_with('.')
        || name.ends_with(['.', ' '])
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}
