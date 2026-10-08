//! ECMA-376 ST_Xstring escapes encode UTF-16 code units, including escaped
//! underscores. Decode once: `_x005F_x0041_` denotes the literal `_x0041_`.

use anyhow::{Context, Result};

pub(crate) fn decode(text: &str) -> Result<String> {
    let mut units = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let bytes = rest.as_bytes();
        if bytes.len() >= 7
            && bytes.starts_with(b"_x")
            && bytes[6] == b'_'
            && bytes[2..6].iter().all(u8::is_ascii_hexdigit)
        {
            units.push(u16::from_str_radix(&rest[2..6], 16)?);
            rest = &rest[7..];
        } else {
            let c = rest.chars().next().unwrap();
            units.extend_from_slice(c.encode_utf16(&mut [0; 2]));
            rest = &rest[c.len_utf8()..];
        }
    }
    String::from_utf16(&units).context("invalid UTF-16 in an Excel string escape")
}
