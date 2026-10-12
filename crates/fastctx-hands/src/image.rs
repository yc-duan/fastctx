//! Images: telling a file's image format and size from its header.
//!
//! The size of a PNG, GIF or WebP image sits at a fixed place near the start.
//! A JPEG's sits in its frame header, after any number of segments that may
//! come first: a phone's EXIF block with its thumbnail, ICC profiles, XMP and
//! depth maps spread over many segments. So the header is read by walking
//! the file segment by segment, however far the frame header is.

use std::io::{self, BufRead as _, BufReader, Read, Seek, SeekFrom};

/// What the start of a file says it is, as an image.
#[derive(Debug, PartialEq, Eq)]
pub enum Header {
    /// A PNG, JPEG, GIF or WebP image, the formats shown as images, and its
    /// size as its header gives it.
    Shown { mime: &'static str, width: u32, height: u32 },
    /// A file whose signature is one of those formats but whose header cannot
    /// be read: what was found, and where.
    Unreadable { format: &'static str, reason: String },
    /// An image in another format, named.
    Other(&'static str),
}

/// The format and size of a PNG, JPEG, GIF or WebP image, from bytes that
/// begin a file and are long enough to reach its size.
pub fn sniff(bytes: &[u8]) -> Option<(&'static str, u32, u32)> {
    match header(io::Cursor::new(bytes)) {
        Ok(Some(Header::Shown { mime, width, height })) => Some((mime, width, height)),
        _ => None,
    }
}

/// Reads as much of `source`, from its start, as it takes to tell what kind
/// of image it is and how large; `None` when it does not begin as an image.
pub fn header(source: impl Read + Seek) -> io::Result<Option<Header>> {
    let mut source = Source::new(source)?;
    let start = source.read_at_most(0, 32)?;
    // A start-of-image marker and the first byte of the next marker, as the
    // WHATWG MIME sniffing standard and libmagic take a JPEG.
    if start.starts_with(b"\xff\xd8\xff") {
        return Ok(Some(match jpeg_size(&mut source)? {
            Ok((width, height)) => Header::Shown { mime: "image/jpeg", width, height },
            Err(reason) => Header::Unreadable { format: "JPEG", reason },
        }));
    }
    if start.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok(Some(fixed("PNG", "image/png", png_size(&start))));
    }
    if start.starts_with(b"GIF87a") || start.starts_with(b"GIF89a") {
        return Ok(Some(fixed("GIF", "image/gif", gif_size(&start))));
    }
    if start.starts_with(b"RIFF") && start.get(8..12) == Some(b"WEBP") {
        return Ok(Some(fixed("WebP", "image/webp", webp_size(&start))));
    }
    Ok(other(&mut source, &start)?.map(Header::Other))
}

fn fixed(format: &'static str, mime: &'static str, size: Result<(u32, u32), String>) -> Header {
    match size {
        Ok((width, height)) => Header::Shown { mime, width, height },
        Err(reason) => Header::Unreadable { format, reason },
    }
}

/// A file read at given offsets through a buffer, so that walking many
/// small segments costs no system call for each.
struct Source<R> {
    reader: BufReader<R>,
    position: u64,
    length: u64,
}

impl<R: Read + Seek> Source<R> {
    fn new(mut source: R) -> io::Result<Self> {
        let length = source.seek(SeekFrom::End(0))?;
        source.seek(SeekFrom::Start(0))?;
        Ok(Self { reader: BufReader::new(source), position: 0, length })
    }

    fn seek(&mut self, at: u64) -> io::Result<()> {
        if at != self.position {
            let delta = i64::try_from(i128::from(at) - i128::from(self.position)).map_err(io::Error::other)?;
            self.reader.seek_relative(delta)?;
            self.position = at;
        }
        Ok(())
    }

    /// The bytes at `at`, fewer than `count` only where the file ends.
    fn read_at_most(&mut self, at: u64, count: usize) -> io::Result<Vec<u8>> {
        self.seek(at)?;
        let mut bytes = Vec::with_capacity(count);
        (&mut self.reader).take(count as u64).read_to_end(&mut bytes)?;
        self.position += bytes.len() as u64;
        Ok(bytes)
    }

    /// Where the first 0xFF byte at or after `at` is, if any.
    fn next_ff(&mut self, at: u64) -> io::Result<Option<u64>> {
        self.seek(at)?;
        loop {
            let buffer = self.reader.fill_buf()?;
            if buffer.is_empty() {
                return Ok(None);
            }
            let skipped = memchr::memchr(0xff, buffer).unwrap_or(buffer.len());
            let found = skipped < buffer.len();
            self.reader.consume(skipped);
            self.position += skipped as u64;
            if found {
                return Ok(Some(self.position));
            }
        }
    }
}

fn png_size(start: &[u8]) -> Result<(u32, u32), String> {
    // IHDR is the first chunk of every PNG, and its first eight bytes are the size.
    let Some(chunk) = start.get(12..16) else {
        return Err(format!("the file ends at byte {} before its first chunk", start.len()));
    };
    if chunk != b"IHDR" {
        return Err(format!("its first chunk is {}, not IHDR", printable(chunk)));
    }
    let (Some(width), Some(height)) = (be32(start, 16), be32(start, 20)) else {
        return Err(format!("the file ends at byte {} inside IHDR", start.len()));
    };
    positive(width, height, "IHDR")
}

fn gif_size(start: &[u8]) -> Result<(u32, u32), String> {
    let (Some(width), Some(height)) = (le16(start, 6), le16(start, 8)) else {
        return Err(format!("the file ends at byte {} inside its screen descriptor", start.len()));
    };
    Ok((width, height))
}

fn webp_size(start: &[u8]) -> Result<(u32, u32), String> {
    let ended = || format!("the file ends at byte {} inside its first chunk", start.len());
    let data = 20;
    let byte = |at: usize| start.get(at).copied().map(u32::from);
    match start.get(12..16) {
        Some(b"VP8 ") => {
            if start.get(data + 3..data + 6).ok_or_else(ended)? != b"\x9d\x01\x2a" {
                return Err("its VP8 frame has no start code".into());
            }
            let width = le16(start, data + 6).ok_or_else(ended)? & 0x3fff;
            let height = le16(start, data + 8).ok_or_else(ended)? & 0x3fff;
            positive(width, height, "the VP8 frame header")
        }
        Some(b"VP8L") => {
            if byte(data) != Some(0x2f) {
                return Err("its VP8L bitstream has no signature".into());
            }
            let b = start.get(data + 1..data + 5).ok_or_else(ended)?;
            let bits = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            Ok(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        Some(b"VP8X") => {
            let b = start.get(data + 4..data + 10).ok_or_else(ended)?;
            let width = u32::from_le_bytes([b[0], b[1], b[2], 0]) + 1;
            let height = u32::from_le_bytes([b[3], b[4], b[5], 0]) + 1;
            Ok((width, height))
        }
        Some(chunk) => Err(format!("its first chunk is {}, not VP8, VP8L or VP8X", printable(chunk))),
        None => Err(ended()),
    }
}

/// Walks a JPEG's segments from after its start-of-image marker to the
/// frame header, which holds its size. The outer error is the file's; the
/// inner says where the walk found what a JPEG cannot hold.
fn jpeg_size(source: &mut Source<impl Read + Seek>) -> io::Result<Result<(u32, u32), String>> {
    let mut stray = None;
    let walked = walk_jpeg(source, &mut stray)?;
    Ok(walked.map_err(|reason| match stray {
        Some(at) => format!("{reason}, after bytes from byte {at} that begin no segment"),
        None => reason,
    }))
}

/// Bytes where a segment should begin are skipped up to the next marker, as
/// libjpeg, and so every browser, does: some writers leave a few there, and
/// the image still opens everywhere. `stray` keeps where the first were.
fn walk_jpeg(source: &mut Source<impl Read + Seek>, stray: &mut Option<u64>) -> io::Result<Result<(u32, u32), String>> {
    let mut at = 2;
    loop {
        let marker = source.read_at_most(at, 2)?;
        match marker[..] {
            [] | [_] => {
                return Ok(Err(format!("the file ends at byte {} before a frame header", source.length)));
            }
            [first, _] if first != 0xff => {
                stray.get_or_insert(at);
                match source.next_ff(at)? {
                    Some(next) => at = next,
                    None => return Ok(Err(format!("the file ends at byte {} before a frame header", source.length))),
                }
                continue;
            }
            // Any number of 0xFF may pad the space before a marker.
            [_, 0xff] => {
                at += 1;
                continue;
            }
            // Markers that stand alone, without a length.
            [_, 0x01 | 0xd0..=0xd7] => {
                at += 2;
                continue;
            }
            // 0xFF00 is an escaped 0xFF inside compressed data, not a marker.
            [_, 0x00] => {
                stray.get_or_insert(at);
                at += 2;
                continue;
            }
            [_, 0xd8] => {
                return Ok(Err(format!("a second start of image is at byte {at}, before any frame header")));
            }
            [_, code @ (0xd9 | 0xda)] => {
                let what = if code == 0xd9 { "the image ends" } else { "its compressed data begins" };
                return Ok(Err(format!("{what} at byte {at}, before any frame header")));
            }
            [_, code] => {
                let name = segment_name(code);
                let length = source.read_at_most(at + 2, 2)?;
                let [high, low] = length[..] else {
                    return Ok(Err(format!(
                        "the file ends at byte {} inside the {name} segment at byte {at}",
                        source.length
                    )));
                };
                let length = u64::from(u16::from_be_bytes([high, low]));
                if length < 2 {
                    return Ok(Err(format!("the {name} segment at byte {at} gives a length of {length}")));
                }
                if at + 2 + length > source.length {
                    return Ok(Err(format!(
                        "the {name} segment at byte {at} runs {} bytes past the end of the file",
                        at + 2 + length - source.length
                    )));
                }
                // Start-of-frame markers carry the size; C4, C8 and CC share
                // the range but are not frames.
                if (0xc0..=0xcf).contains(&code) && !matches!(code, 0xc4 | 0xc8 | 0xcc) {
                    let frame = source.read_at_most(at + 4, 5)?;
                    if length < 8 || frame.len() < 5 {
                        return Ok(Err(format!(
                            "the frame header at byte {at} is {length} bytes long, too short to hold a size"
                        )));
                    }
                    let height = u32::from(u16::from_be_bytes([frame[1], frame[2]]));
                    let width = u32::from(u16::from_be_bytes([frame[3], frame[4]]));
                    return Ok(positive(width, height, &format!("the frame header at byte {at}")));
                }
                at += 2 + length;
            }
            [_, _, _, ..] => unreachable!("two bytes were read at most"),
        }
    }
}

fn segment_name(code: u8) -> String {
    match code {
        0xe0..=0xef => format!("APP{}", code - 0xe0),
        0xc0..=0xcf if !matches!(code, 0xc4 | 0xc8 | 0xcc) => format!("SOF{}", code - 0xc0),
        0xc4 => "DHT".into(),
        0xcc => "DAC".into(),
        0xdb => "DQT".into(),
        0xdd => "DRI".into(),
        0xfe => "COM".into(),
        _ => format!("0xFF{code:02X}"),
    }
}

fn positive(width: u32, height: u32, place: &str) -> Result<(u32, u32), String> {
    if width == 0 || height == 0 {
        return Err(format!("{place} gives a size of {width}x{height}"));
    }
    Ok((width, height))
}

/// An image format that is not shown, by its signature. Each signature holds
/// bytes no text begins with (NUL, other controls, or 0xFF), so a text file is
/// not taken for an image.
fn other(source: &mut Source<impl Read + Seek>, start: &[u8]) -> io::Result<Option<&'static str>> {
    let starts = |signature: &[u8]| start.starts_with(signature);
    if start.get(4..8) == Some(b"ftyp") {
        return iso_image(source, start);
    }
    let found = if starts(b"II*\0") || starts(b"MM\0*") || starts(b"II+\0") || starts(b"MM\0+") {
        Some("TIFF")
    } else if starts(b"BM")
        && start.get(6..10) == Some(&[0; 4])
        && matches!(le32(start, 14), Some(12 | 40 | 52 | 56 | 64 | 108 | 124))
    {
        Some("BMP")
    } else if (starts(b"\0\0\x01\0") || starts(b"\0\0\x02\0"))
        && le16(start, 4).is_some_and(|count| count > 0)
        && start.get(9) == Some(&0)
    {
        Some("ICO")
    } else if starts(b"\xff\x0a") || starts(b"\0\0\0\x0cJXL \r\n\x87\n") {
        Some("JPEG XL")
    } else if starts(b"\0\0\0\x0cjP  \r\n\x87\n") || starts(b"\xff\x4f\xff\x51") {
        Some("JPEG 2000")
    } else if starts(b"8BPS\0\x01") || starts(b"8BPS\0\x02") {
        Some("Photoshop")
    } else if starts(b"qoif") && matches!(start.get(12..14), Some([3 | 4, 0 | 1])) {
        Some("QOI")
    } else if starts(b"\x76\x2f\x31\x01") {
        Some("OpenEXR")
    } else {
        None
    };
    Ok(found)
}

/// HEIF, HEIC, AVIF and Canon's CR3 raw files are ISO media files, as videos
/// are; their brands, in the leading `ftyp` box, tell them apart.
fn iso_image(source: &mut Source<impl Read + Seek>, start: &[u8]) -> io::Result<Option<&'static str>> {
    let Some(size) = be32(start, 0) else { return Ok(None) };
    let end = u64::from(size).min(source.length);
    // Ranked: an AVIF file names mif1 too, and an HEIC file mif1 or msf1.
    let families: [(&[&[u8; 4]], &str); 4] = [
        (&[b"avif", b"avis"], "AVIF"),
        (&[b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"hevm", b"hevs"], "HEIC"),
        (&[b"mif1", b"msf1"], "HEIF"),
        (&[b"crx "], "Canon CR3 raw"),
    ];
    let mut best: Option<usize> = None;
    // The major brand, then the compatible ones after the minor version.
    for at in std::iter::once(8).chain((16..end).step_by(4)) {
        let brand = source.read_at_most(at, 4)?;
        if brand.len() < 4 {
            break;
        }
        if let Some(rank) = families.iter().position(|(names, _)| names.iter().any(|name| brand[..] == name[..])) {
            best = Some(best.map_or(rank, |best| best.min(rank)));
        }
    }
    Ok(best.map(|rank| families[rank].1))
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    bytes.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn le16(bytes: &[u8], at: usize) -> Option<u32> {
    bytes.get(at..at + 2).map(|b| u32::from(u16::from_le_bytes([b[0], b[1]])))
}

/// Four bytes of a chunk name as text, each other byte as `\xNN`.
fn printable(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&byte| {
            if byte.is_ascii_graphic() || byte == b' ' {
                char::from(byte).to_string()
            } else {
                format!("\\x{byte:02X}")
            }
        })
        .collect()
}
