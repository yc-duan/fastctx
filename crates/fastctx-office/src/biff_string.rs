//! BIFF8 formula STRING records, including compression changes at CONTINUE.
use anyhow::{Context, Result, ensure};

pub(crate) fn read(parts: &[&[u8]]) -> Result<String> {
    let mut data = Vec::new();
    let mut boundaries = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            boundaries.push(data.len());
        }
        data.extend_from_slice(part);
    }
    let mut input = crate::formula::Bytes(&data);
    let count = input.u16()? as usize;
    ensure!(count <= 32767, "formula string exceeds the BIFF8 character limit");
    let mut wide = input.u8()? & 1 != 0;
    let mut at = 3;
    let mut boundaries = boundaries.into_iter().filter(|&p| p >= 3).peekable();
    let mut units = Vec::with_capacity(count);
    while units.len() < count {
        if boundaries.peek() == Some(&at) {
            wide = data.get(at).context("missing STRING continuation flags")? & 1 != 0;
            at += 1;
            boundaries.next();
        }
        let width = if wide { 2 } else { 1 };
        ensure!(boundaries.peek().is_none_or(|&end| at + width <= end), "split UTF-16 code unit in STRING");
        let bytes = data.get(at..at + width).context("truncated formula string cache")?;
        units.push(if wide { u16::from_le_bytes([bytes[0], bytes[1]]) } else { bytes[0].into() });
        at += width;
    }
    ensure!(at == data.len(), "extra bytes after formula string cache");
    String::from_utf16(&units).context("invalid UTF-16 in formula string cache")
}
