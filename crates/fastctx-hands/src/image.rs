//! Images: telling a supported format and its size from the header.

/// The format and size of an image, from its header.
pub fn sniff(bytes: &[u8]) -> Option<(&'static str, u32, u32)> {
    let be32 = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let le16 = |at: usize| bytes.get(at..at + 2).map(|b| u32::from(u16::from_le_bytes([b[0], b[1]])));
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.get(12..16) == Some(b"IHDR") {
        return Some(("image/png", be32(16)?, be32(20)?));
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(("image/gif", le16(6)?, le16(8)?));
    }
    if bytes.starts_with(b"\xff\xd8") {
        return jpeg_size(bytes).map(|(width, height)| ("image/jpeg", width, height));
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return webp_size(bytes).map(|(width, height)| ("image/webp", width, height));
    }
    None
}

fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2;
    loop {
        while *bytes.get(at)? == 0xff && *bytes.get(at + 1)? == 0xff {
            at += 1;
        }
        if *bytes.get(at)? != 0xff {
            return None;
        }
        let marker = *bytes.get(at + 1)?;
        let length = usize::from(u16::from_be_bytes([*bytes.get(at + 2)?, *bytes.get(at + 3)?]));
        // Start-of-frame markers carry the size; C4, C8 and CC share the
        // range but are not frames.
        if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            let height = u32::from(u16::from_be_bytes([*bytes.get(at + 5)?, *bytes.get(at + 6)?]));
            let width = u32::from(u16::from_be_bytes([*bytes.get(at + 7)?, *bytes.get(at + 8)?]));
            return Some((width, height));
        }
        at += 2 + length;
    }
}

fn webp_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let chunk = bytes.get(12..16)?;
    let data = 20;
    match chunk {
        b"VP8 " => {
            let width = u32::from(u16::from_le_bytes([*bytes.get(data + 6)?, *bytes.get(data + 7)?]) & 0x3fff);
            let height = u32::from(u16::from_le_bytes([*bytes.get(data + 8)?, *bytes.get(data + 9)?]) & 0x3fff);
            Some((width, height))
        }
        b"VP8L" => {
            let b = bytes.get(data + 1..data + 5)?;
            let bits = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        b"VP8X" => {
            let width =
                u32::from_le_bytes([*bytes.get(data + 4)?, *bytes.get(data + 5)?, *bytes.get(data + 6)?, 0]) + 1;
            let height =
                u32::from_le_bytes([*bytes.get(data + 7)?, *bytes.get(data + 8)?, *bytes.get(data + 9)?, 0]) + 1;
            Some((width, height))
        }
        _ => None,
    }
}
