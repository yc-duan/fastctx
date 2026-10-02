//! Streaming terminal scanner. Image payloads stay in the log until selected for presentation.

use super::ByteRange;
use super::window::{Body, Piece, Protocol, Window};
use std::io::{self, BufRead, BufReader, Read};

pub(super) struct Scan<R> {
    reader: BufReader<R>,
    pub offset: u64,
    replay: Option<u8>,
}

impl<R: Read> Scan<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader: BufReader::with_capacity(64 * 1024, reader),
            offset: 0,
            replay: None,
        }
    }

    pub fn byte(&mut self) -> io::Result<Option<u8>> {
        if let Some(byte) = self.replay.take() {
            self.offset += 1;
            return Ok(Some(byte));
        }
        let data = self.reader.fill_buf()?;
        let byte = data.first().copied();
        if byte.is_some() {
            self.reader.consume(1);
            self.offset += 1;
        }
        Ok(byte)
    }

    pub fn peek(&mut self) -> io::Result<Option<u8>> {
        if self.replay.is_some() {
            Ok(self.replay)
        } else {
            Ok(self.reader.fill_buf()?.first().copied())
        }
    }

    /// Calls the sink for payload bytes, excluding the terminator. No sequence length limit.
    pub fn string(&mut self, osc: bool, mut sink: impl FnMut(u8)) -> io::Result<bool> {
        loop {
            match self.byte()? {
                None => return Ok(false),
                Some(7) if osc => return Ok(true),
                Some(0x9c) => return Ok(true),
                Some(0xc2) if self.peek()? == Some(0x9c) => {
                    self.byte()?;
                    return Ok(true);
                }
                Some(0x1b) => {
                    if self.peek()? == Some(b'\\') {
                        self.byte()?;
                        return Ok(true);
                    }
                    // ESC cancels a damaged string; leave the following sequence parseable.
                    self.offset -= 1;
                    self.replay = Some(0x1b);
                    return Ok(false);
                }
                Some(byte) => sink(byte),
            }
        }
    }

    pub fn header(&mut self, delimiter: u8, expected: &[u8]) -> io::Result<(bool, bool)> {
        let mut index = 0;
        let mut matched = true;
        loop {
            match self.peek()? {
                Some(byte) if byte == delimiter => {
                    self.byte()?;
                    return Ok((matched && index >= expected.len(), true));
                }
                Some(0x1b | 7) | None => return Ok((matched && index >= expected.len(), false)),
                Some(_) => {
                    let byte = self.byte()?.expect("peek found a byte");
                    if index < expected.len() && expected[index] != byte {
                        matched = false;
                    }
                    index += 1;
                }
            }
        }
    }
}

#[derive(Default, Debug)]
pub(super) struct KittyHeader {
    pub more: bool,
    pub png: bool,
    pub direct: bool,
    pub compressed: bool,
    pub continuation: bool,
    pub display: bool,
}

#[derive(Clone, Copy)]
pub(super) enum Osc {
    File,
    Multipart,
    Part,
    End,
    Other,
}

pub(super) fn osc<R: Read>(scan: &mut Scan<R>) -> io::Result<Osc> {
    let keys: [&[u8]; 4] = [
        b"1337;File=",
        b"1337;MultipartFile=",
        b"1337;FilePart=",
        b"1337;FileEnd",
    ];
    let kinds = [Osc::File, Osc::Multipart, Osc::Part, Osc::End];
    let mut candidates = [true; 4];
    let mut index = 0;
    while let Some(byte) = scan.peek()? {
        if matches!(byte, 7 | 0x1b) {
            break;
        }
        scan.byte()?;
        for ((key, candidate), kind) in keys.iter().zip(&mut candidates).zip(kinds) {
            *candidate &= key.get(index) == Some(&byte);
            if *candidate && index + 1 == key.len() {
                return Ok(kind);
            }
        }
        if !candidates.iter().any(|candidate| *candidate) {
            break;
        }
        index += 1;
    }
    Ok(Osc::Other)
}

pub(super) fn kitty_header<R: Read>(scan: &mut Scan<R>) -> io::Result<(KittyHeader, bool)> {
    let mut header = KittyHeader {
        direct: true,
        continuation: true,
        display: true,
        ..Default::default()
    };
    let mut key = None;
    let mut value = 0_u32;
    let mut first = None;
    let mut in_value = false;
    loop {
        let Some(byte) = scan.peek()? else {
            return Ok((header, false));
        };
        if byte == 0x1b || byte == 7 {
            return Ok((header, false));
        }
        scan.byte()?;
        if byte == b',' || byte == b';' {
            match key {
                Some(b'm') => header.more = value == 1,
                Some(b'f') => {
                    header.png = value == 100;
                    header.continuation = false;
                }
                Some(b't') => {
                    header.direct = first == Some(b'd');
                    header.continuation = false;
                }
                Some(b'o') => {
                    header.compressed = first == Some(b'z');
                    header.continuation = false;
                }
                Some(b'a') => {
                    header.display = matches!(first, Some(b't' | b'T'));
                    header.continuation = false;
                }
                Some(b'q') | None => (),
                _ => header.continuation = false,
            }
            if byte == b';' {
                return Ok((header, true));
            }
            key = None;
            first = None;
            value = 0;
            in_value = false;
        } else if byte == b'=' {
            in_value = true;
        } else if in_value {
            if first.is_none() {
                first = Some(byte);
            }
            value = value
                .saturating_mul(10)
                .saturating_add(u32::from(byte.saturating_sub(b'0')));
        } else if key.is_none() {
            key = Some(byte);
        }
    }
}

pub(super) fn scan<R: Read>(reader: R) -> io::Result<Window> {
    let mut scan = Scan::new(reader);
    let mut window = Window::default();
    let mut pending_kitty = None;
    let mut pending_iterm = None;
    while let Some(mut byte) = scan.peek()? {
        let start = scan.offset;
        if byte == 0xc2 {
            scan.byte()?;
            if scan
                .peek()?
                .is_some_and(|next| (0x80..=0x9f).contains(&next))
            {
                byte = scan.byte()?.unwrap();
            } else {
                utf8(&mut scan, &mut window, start, byte)?;
                continue;
            }
        } else if byte != 0x1b
            && !(0x7f..=0x9f).contains(&byte)
            && (byte >= 0x20 || matches!(byte, b'\n' | b'\t'))
        {
            text(&mut scan, &mut window)?;
            continue;
        } else {
            scan.byte()?;
        }
        let control = match byte {
            0x1b => scan.byte()?,
            0x9b => Some(b'['),
            0x9d => Some(b']'),
            0x9f => Some(b'_'),
            0x90 => Some(b'P'),
            0x98 => Some(b'X'),
            0x9e => Some(b'^'),
            _ => None,
        };
        match control {
            Some(b'[') => {
                while let Some(byte) = scan.peek()? {
                    if byte == 0x1b {
                        break;
                    }
                    scan.byte()?;
                    if (0x40..=0x7e).contains(&byte) {
                        break;
                    }
                }
            }
            Some(b']') => match osc(&mut scan)? {
                Osc::File => {
                    if let Some(previous) = pending_iterm.take() {
                        push_image(&mut window, previous, start, Protocol::Iterm, false);
                    }
                    if let Some(previous) = pending_kitty.take() {
                        push_kitty(&mut window, previous, start, false);
                    }
                    let (_, payload) = scan.header(b':', b"")?;
                    let complete = scan.string(true, |_| {})?;
                    push_image(
                        &mut window,
                        start,
                        scan.offset,
                        Protocol::Iterm,
                        complete && payload,
                    );
                }
                Osc::Multipart => {
                    if let Some(previous) = pending_iterm.take() {
                        push_image(&mut window, previous, start, Protocol::Iterm, false);
                    }
                    if scan.string(true, |_| {})? {
                        pending_iterm = Some(start);
                    } else {
                        push_image(&mut window, start, scan.offset, Protocol::Iterm, false);
                    }
                }
                Osc::Part => {
                    let complete = scan.string(true, |_| {})?;
                    if !complete || pending_iterm.is_none() {
                        push_image(
                            &mut window,
                            pending_iterm.take().unwrap_or(start),
                            scan.offset,
                            Protocol::Iterm,
                            false,
                        );
                    }
                }
                Osc::End => {
                    let complete = scan.string(true, |_| {})?;
                    let previous = pending_iterm.take();
                    push_image(
                        &mut window,
                        previous.unwrap_or(start),
                        scan.offset,
                        Protocol::Iterm,
                        complete && previous.is_some(),
                    );
                }
                Osc::Other => {
                    scan.string(true, |_| {})?;
                }
            },
            Some(b'_') if scan.peek()? == Some(b'G') => {
                scan.byte()?;
                let (header, payload) = kitty_header(&mut scan)?;
                let complete = scan.string(false, |_| {})?;
                if pending_kitty.is_some() && !header.continuation {
                    push_kitty(&mut window, pending_kitty.take().unwrap(), start, false);
                }
                if header.display || pending_kitty.is_some() {
                    let image_start = pending_kitty.take().unwrap_or(start);
                    if header.more && complete && payload {
                        pending_kitty = Some(image_start);
                    } else {
                        push_kitty(&mut window, image_start, scan.offset, complete && payload);
                    }
                }
            }
            Some(b'P' | b'X' | b'^' | b'_') => {
                scan.string(false, |_| {})?;
            }
            Some(0x20..=0x2f) => {
                while let Some(byte) = scan.peek()? {
                    if byte == 0x1b {
                        break;
                    }
                    scan.byte()?;
                    if (0x30..=0x7e).contains(&byte) {
                        break;
                    }
                }
            }
            _ => (),
        }
    }
    if let Some(start) = pending_kitty {
        push_kitty(&mut window, start, scan.offset, false);
    }
    if let Some(start) = pending_iterm {
        push_image(&mut window, start, scan.offset, Protocol::Iterm, false);
    }
    Ok(window)
}

fn push_kitty(window: &mut Window, start: u64, end: u64, complete: bool) {
    push_image(window, start, end, Protocol::Kitty, complete);
}

fn push_image(window: &mut Window, start: u64, end: u64, protocol: Protocol, complete: bool) {
    window.push(Piece {
        range: ByteRange { start, end },
        body: Body::Image {
            protocol,
            complete,
            source: ByteRange { start, end },
        },
    });
}

fn text<R: Read>(scan: &mut Scan<R>, window: &mut Window) -> io::Result<()> {
    let start = scan.offset;
    let buffer = scan.reader.fill_buf()?;
    let length = control_offset(buffer);
    match std::str::from_utf8(&buffer[..length]) {
        Ok(text) => {
            window.push(Piece {
                range: ByteRange {
                    start,
                    end: start + length as u64,
                },
                body: Body::Text(text.to_owned()),
            });
            scan.reader.consume(length);
            scan.offset += length as u64;
        }
        Err(error) if error.valid_up_to() > 0 => {
            let valid = error.valid_up_to();
            let text = std::str::from_utf8(&buffer[..valid]).expect("validated prefix");
            window.push(Piece {
                range: ByteRange {
                    start,
                    end: start + valid as u64,
                },
                body: Body::Text(text.to_owned()),
            });
            scan.reader.consume(valid);
            scan.offset += valid as u64;
        }
        Err(_) => {
            let first = scan.byte()?.expect("text is nonempty");
            utf8(scan, window, start, first)?;
        }
    }
    Ok(())
}

fn control_offset(buffer: &[u8]) -> usize {
    unsafe extern "C" {
        #[link_name = "memchr"]
        fn find_byte(
            source: *const std::ffi::c_void,
            byte: std::ffi::c_int,
            count: usize,
        ) -> *mut std::ffi::c_void;
    }
    // The platform C runtime supplies an optimized scan even in a debug build. Keep
    // tabs and LF as text; C2 can introduce UTF-8 C1 controls and is inspected separately.
    let mut length = buffer.len();
    for byte in (0_u8..=31)
        .filter(|byte| !matches!(*byte, 9 | 10))
        .chain([127, 194])
    {
        // SAFETY: memchr reads at most length bytes from this live slice. A nonnull
        // result points into that same slice, so the offset is nonnegative and in bounds.
        let found = unsafe { find_byte(buffer.as_ptr().cast(), byte.into(), length) };
        if !found.is_null() {
            length = unsafe { found.cast::<u8>().offset_from(buffer.as_ptr()) } as usize;
        }
    }
    length
}

fn utf8<R: Read>(scan: &mut Scan<R>, window: &mut Window, start: u64, first: u8) -> io::Result<()> {
    let mut encoded = vec![first];
    let needed = match first {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => 1,
    };
    while encoded.len() < needed && scan.peek()?.is_some_and(|next| next & 0xc0 == 0x80) {
        encoded.push(scan.byte()?.unwrap());
    }
    let text = match std::str::from_utf8(&encoded) {
        Ok(text) => text.to_owned(),
        Err(_) => format!(
            "[invalid UTF-8 at byte {start}: {}]",
            encoded
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ),
    };
    window.push(Piece {
        range: ByteRange {
            start,
            end: scan.offset,
        },
        body: Body::Text(text),
    });
    Ok(())
}
