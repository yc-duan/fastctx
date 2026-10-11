//! Whole-line decisions over replayable byte ranges; only decoded chunks reside in RAM.

use std::io::{self, BufRead, BufReader, Read, Write};

use super::file::Plan;
use super::statistics::{BASIS, Statistics};
use super::{EncodingError, TextDecoder, TextEncoding};
use crate::spool::{BLOCK, Bytes, Spool};

type Result<T> = std::result::Result<T, String>;
fn io(error: io::Error) -> String {
    error.to_string()
}

pub(crate) struct Line {
    pub bytes: std::ops::Range<usize>,
    pub text: Bytes,
    pub codec: Option<TextEncoding>,
    pub basis: &'static str,
    pub issue: Option<String>,
}

pub(crate) fn visit(reader: impl Read, plan: &Plan, mut consume: impl FnMut(Line) -> Result<()>) -> Result<()> {
    let mut reader = BufReader::with_capacity(BLOCK, reader);
    let bom = plan.signature().map_or(0, |c| c.bom_len());
    let mut signature = [0; 4];
    reader.read_exact(&mut signature[..bom]).map_err(io)?;
    let mut offset = bom;
    let unit = plan.fixed.map_or(1, |c| c.unit());
    let newline = plan.fixed.map(|c| c.encode("\n").expect("newline")).unwrap_or_else(|| vec![b'\n']);
    let crlf = plan.fixed.map(|c| c.encode("\r\n").expect("newline")).unwrap_or_else(|| b"\r\n".to_vec());
    let mut previous = plan.fixed.or_else(|| plan.legacy.first().copied()).unwrap_or_else(TextEncoding::utf8);
    let mut stateful = plan.fixed.filter(|c| c.is_stateful()).map(|c| TextDecoder::new(c.without_bom()));
    let mut state_error: Option<String> = None;
    loop {
        let mut raw = Spool::default();
        if unit == 1 {
            loop {
                let bytes = reader.fill_buf().map_err(io)?;
                if bytes.is_empty() {
                    break;
                }
                let n = memchr::memchr(b'\n', bytes).map_or(bytes.len(), |at| at + 1);
                let ended = bytes[n - 1] == b'\n';
                raw.write_all(&bytes[..n]).map_err(io)?;
                reader.consume(n);
                if ended {
                    break;
                }
            }
        } else {
            loop {
                let mut bytes = [0; 4];
                let mut n = 0;
                while n < unit {
                    match reader.read(&mut bytes[n..unit]) {
                        Ok(0) => break,
                        Ok(read) => n += read,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                        Err(e) => return Err(io(e)),
                    }
                }
                raw.write_all(&bytes[..n]).map_err(io)?;
                if n < unit || bytes[..n] == newline {
                    break;
                }
            }
        }
        if raw.len() == 0 {
            break;
        }
        let raw = raw.finish();
        let end = reader.fill_buf().map_err(io)?.is_empty();
        let mut line = if let Some(decoder) = stateful.as_mut() {
            match decode(&raw, decoder, end)? {
                Ok(text) => Line { bytes: 0..0, text, codec: plan.fixed, basis: plan.basis, issue: None },
                Err(error) => {
                    let error = match error {
                        EncodingError::Malformed { encoding, offset: failed } => {
                            EncodingError::Malformed { encoding, offset: failed.saturating_sub(offset) }
                        }
                        error => error,
                    };
                    let reason = match &state_error {
                        Some(reason) => reason.clone(),
                        None => {
                            let reason = describe(&error, &raw, offset)?;
                            state_error = Some(reason.clone());
                            reason
                        }
                    };
                    readonly(&raw, reason)?
                }
            }
        } else {
            choose(&raw, offset, plan, end, previous)?
        };
        if line.issue.is_some() {
            let (removed, ending) = if raw.ends_with(&crlf).map_err(io)? {
                (crlf.len(), "\r\n")
            } else if raw.ends_with(&newline).map_err(io)? {
                (newline.len(), "\n")
            } else {
                (0, "")
            };
            line.text = preview(&raw.slice(0..raw.len() - removed), true, ending)?;
        }
        if let Some(codec) = line.codec {
            previous = codec;
        }
        line.bytes = offset..offset + raw.len();
        offset += raw.len();
        consume(line)?;
    }
    Ok(())
}

pub(crate) fn decode(
    raw: &Bytes,
    decoder: &mut TextDecoder,
    end: bool,
) -> Result<std::result::Result<Bytes, EncodingError>> {
    let mut text = Spool::default();
    for bytes in raw.chunks() {
        match decoder.feed(&bytes.map_err(io)?, false) {
            Ok(chunk) => text.write_all(chunk.as_bytes()).map_err(io)?,
            Err(error) => return Ok(Err(error)),
        }
    }
    match decoder.feed(&[], end) {
        Ok(chunk) => {
            text.write_all(chunk.as_bytes()).map_err(io)?;
            Ok(Ok(text.finish()))
        }
        Err(error) => Ok(Err(error)),
    }
}

fn reading(raw: &Bytes, codec: TextEncoding) -> Result<std::result::Result<Bytes, EncodingError>> {
    if codec.without_bom() == TextEncoding::utf8()
        && raw.as_slice().is_some_and(|bytes| std::str::from_utf8(bytes).is_ok())
    {
        return Ok(Ok(raw.clone()));
    }
    decode(raw, &mut TextDecoder::new(codec.without_bom()), true)
}
fn decoded(text: Bytes, codec: TextEncoding, basis: &'static str) -> Line {
    Line { bytes: 0..0, text, codec: Some(codec.without_bom()), basis, issue: None }
}
fn describe(error: &EncodingError, raw: &Bytes, offset: usize) -> Result<String> {
    if let EncodingError::Malformed { encoding, offset: at } = error {
        let byte = raw.byte((*at).min(raw.len())).map_err(io)?;
        let hex = byte.map_or_else(|| "EOF".into(), |b| format!("{b:02X}"));
        return Ok(format!(
            "{encoding} does not read the bytes from byte {} (hex {hex}), so this text cannot be edited; xxd shows the bytes, or view the file with --encoding to name its encoding",
            offset + at
        ));
    }
    Ok(error.to_string())
}
fn readonly(raw: &Bytes, reason: String) -> Result<Line> {
    Ok(Line {
        bytes: 0..0,
        text: preview(raw, false, "")?,
        codec: None,
        basis: "read-only byte preview",
        issue: Some(reason),
    })
}

fn iso(raw: &Bytes) -> Result<(bool, super::iso2022::State)> {
    let mut found = false;
    let mut tail = Vec::new();
    let mut state = super::iso2022::State::Ascii;
    for bytes in raw.chunks() {
        let bytes = bytes.map_err(io)?;
        tail.extend_from_slice(&bytes);
        // A switch has three bytes; leave two undecided until the next block.
        let n = tail.len().saturating_sub(2);
        let mut at = 0;
        while at < n {
            if tail[at] == 0x1b && at + 3 <= tail.len() {
                let sequence = &tail[at..at + 3];
                if super::iso2022::has_switch(sequence) {
                    found = true;
                    state = state.after(sequence);
                    at += 3;
                    continue;
                }
            }
            at += 1;
        }
        tail.drain(..at);
    }
    found |= super::iso2022::has_switch(&tail);
    state = state.after(&tail);
    Ok((found, state))
}
fn boundary(raw: &Bytes, offset: usize) -> Result<Line> {
    readonly(
        raw,
        format!(
            "ISO-2022-JP leaves a non-ASCII state at the line boundary after byte {} (last hex {:02X}); the next segment's state is unresolved. Select ISO-2022-JP alone if the whole file uses it, or inspect the boundary bytes with xxd",
            offset + raw.len(),
            raw.byte(raw.len() - 1).map_err(io)?.expect("nonempty")
        ),
    )
}
fn statistics(raw: &Bytes) -> Result<Statistics> {
    let mut stats = Statistics::new();
    for bytes in raw.chunks() {
        stats.feed(&bytes.map_err(io)?, false);
    }
    stats.feed(&[], true);
    Ok(stats)
}
fn select(raw: &Bytes, codecs: impl Iterator<Item = TextEncoding>) -> Result<Option<(TextEncoding, Bytes)>> {
    let mut selected: Option<(TextEncoding, Bytes)> = None;
    for codec in codecs {
        if let Ok(text) = reading(raw, codec)? {
            if let Some((_, previous)) = &selected {
                if !previous.equals(&text).map_err(io)? {
                    return Ok(None);
                }
            } else {
                selected = Some((codec, text));
            }
        }
    }
    Ok(selected)
}
fn stable(raw: &Bytes, codec: TextEncoding) -> Result<bool> {
    let mut first = None;
    let mut last = 0;
    let mut offset = 0;
    for bytes in raw.chunks() {
        let bytes = bytes.map_err(io)?;
        for (at, byte) in bytes.iter().enumerate() {
            if !byte.is_ascii() {
                first.get_or_insert(offset + at);
                last = offset + at;
            }
        }
        offset += bytes.len();
    }
    let Some(first) = first else {
        return Ok(false);
    };
    let middle = first + (last + 1 - first) / 2;
    for split in middle..=last {
        if split <= first {
            continue;
        }
        let left = raw.slice(0..split);
        let right = raw.slice(split..raw.len());
        if reading(&left, codec)?.is_ok() && reading(&right, codec)?.is_ok() {
            for part in [&left, &right] {
                if select(part, statistics(part)?.leaders().into_iter())?
                    .is_none_or(|(candidate, _)| candidate != codec)
                {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
    }
    Ok(false)
}

fn choose(raw: &Bytes, offset: usize, plan: &Plan, end: bool, previous: TextEncoding) -> Result<Line> {
    if let Some(codec) = plan.fixed {
        return match reading(raw, codec)? {
            Ok(text) => Ok(decoded(text, codec, plan.basis)),
            Err(error) => readonly(raw, describe(&error, raw, offset)?),
        };
    }
    let (has_switch, state) = iso(raw)?;
    if !plan.ordered && has_switch {
        let codec = super::explicit("iso-2022-jp", &[]).expect("codec");
        return match reading(raw, codec)? {
            Ok(text) if end || state == super::iso2022::State::Ascii => {
                Ok(decoded(text, codec, "ISO-2022-JP escapes and strict line validation"))
            }
            Ok(_) => boundary(raw, offset),
            Err(error) => readonly(raw, describe(&error, raw, offset)?),
        };
    }
    let utf8 = reading(raw, TextEncoding::utf8())?;
    let invalid = match utf8 {
        Ok(text) if !plan.ordered => {
            let ascii = raw.is_ascii().map_err(io)?;
            let codec = if ascii && !previous.is_stateful() { previous } else { TextEncoding::utf8() };
            let basis = if ascii && previous != TextEncoding::utf8() { BASIS } else { "UTF-8" };
            return Ok(decoded(text, codec, basis));
        }
        Err(EncodingError::Malformed { offset, .. }) => offset,
        _ => 0,
    };
    if plan.ordered {
        for &codec in &plan.choices {
            if let Ok(text) = reading(raw, codec)? {
                if codec.is_stateful() && !end && state != super::iso2022::State::Ascii {
                    return boundary(raw, offset);
                }
                return Ok(decoded(text, codec, plan.basis));
            }
        }
    }
    let stats = statistics(raw)?;
    if !plan.ordered {
        let context = select(
            raw,
            plan.legacy.iter().copied().filter(|codec| stats.score(*codec).is_some_and(|score| score > 0)),
        )?;
        let local = select(raw, stats.leaders().into_iter())?;
        let selected = match (context, local) {
            (Some(context), Some(local)) if context.0 != local.0 && stable(raw, local.0)? => Some(local),
            (Some(context), _) => Some(context),
            (None, local) => local,
        };
        if let Some((codec, text)) = selected {
            return Ok(decoded(text, codec, BASIS));
        }
    }
    let mut alternatives: Vec<(TextEncoding, Bytes)> = Vec::new();
    if !plan.ordered {
        for (codec, _) in stats.ranked() {
            if let Ok(text) = reading(raw, codec)? {
                let mut same = false;
                for (_, other) in &alternatives {
                    same |= other.equals(&text).map_err(io)?;
                }
                if !same {
                    alternatives.push((codec, text));
                }
                if alternatives.len() == 3 {
                    break;
                }
            }
        }
    }
    let mut focus = usize::MAX;
    if let Some((_, first)) = alternatives.first() {
        for (_, other) in alternatives.iter().skip(1) {
            let mut common = 0;
            for (a, b) in first.chars().zip(other.chars()) {
                if a.map_err(io)? != b.map_err(io)? {
                    break;
                }
                common += 1;
            }
            focus = focus.min(common);
        }
    }
    if focus == usize::MAX {
        focus = 0;
    }
    let reason = if !alternatives.is_empty() {
        let mut descriptions = Vec::new();
        for (codec, text) in alternatives {
            descriptions.push(format!("{} {:?}", codec.label(), excerpt_at(&content(&text)?, focus)?));
        }
        format!(
            "its encoding is undecided, it reads as {}; view the file with --encoding and one of these to read it as that",
            descriptions.join(", ")
        )
    } else {
        let labels = if plan.ordered {
            plan.choices.iter().map(|c| c.label()).collect::<Vec<_>>().join(", ")
        } else {
            "UTF-8 or any older encoding".into()
        };
        let hex = raw.byte(invalid).map_err(io)?.map_or_else(|| "EOF".into(), |b| format!("{b:02X}"));
        format!(
            "it does not read whole as {labels}, from byte {} (hex {hex}), so it may be damaged; xxd shows the bytes, or view the file with --encoding to name its encoding",
            offset + invalid
        )
    };
    readonly(raw, reason)
}

pub(crate) fn content(text: &Bytes) -> Result<Bytes> {
    let ending = if text.ends_with(b"\r\n").map_err(io)? { 2 } else { usize::from(text.ends_with(b"\n").map_err(io)?) };
    Ok(text.slice(0..text.len() - ending))
}
pub(crate) fn excerpt(text: &Bytes) -> Result<String> {
    let mut previous = None;
    let mut focus = 0;
    for (at, c) in text.chars().enumerate() {
        let c = c.map_err(io)?;
        if previous == Some('\\') && c == 'x' {
            focus = at - 1;
            break;
        }
        previous = Some(c);
    }
    excerpt_at(text, focus)
}
fn excerpt_at(text: &Bytes, focus: usize) -> Result<String> {
    const WIDTH: usize = 80;
    let mut length = 0;
    text.utf8(|text| {
        length += crate::text::visible_length(text);
        Ok(())
    })
    .map_err(io)?;
    if length <= WIDTH {
        return Ok(crate::text::visible(&text.text().map_err(io)?));
    }
    let start = focus.saturating_sub(WIDTH / 2);
    let mut before = 0;
    let mut kept = String::new();
    for (at, c) in text.chars().enumerate() {
        let c = c.map_err(io)?;
        if at < start {
            before += crate::text::visible_length(c.encode_utf8(&mut [0; 4]));
        } else if at - start < WIDTH {
            kept.push(c);
        } else {
            break;
        }
    }
    let mut shown: String = crate::text::visible(&kept).chars().take(WIDTH).collect();
    let after = length - before - shown.chars().count();
    if before > 0 {
        shown = format!("({before} characters before)… {shown}");
    }
    if after > 0 {
        shown.push_str(&format!("… ({after} more characters)"));
    }
    Ok(shown)
}
pub(crate) fn preview(raw: &Bytes, quote_newlines: bool, ending: &str) -> Result<Bytes> {
    let mut spool = Spool::default();
    let mut pending = Vec::new();
    let mut emit = |bytes: &[u8], end| -> Result<()> {
        pending.extend_from_slice(bytes);
        let mut at = 0;
        while at < pending.len() {
            let (valid, invalid) = match std::str::from_utf8(&pending[at..]) {
                Ok(text) => (text.len(), 0),
                Err(error) => (
                    error.valid_up_to(),
                    match error.error_len() {
                        Some(n) => n,
                        None if end => pending.len() - at - error.valid_up_to(),
                        None => 0,
                    },
                ),
            };
            let text = std::str::from_utf8(&pending[at..at + valid]).expect("validated");
            if quote_newlines {
                spool.write_all(text.replace('\r', r"\x0D").replace('\n', r"\x0A").as_bytes()).map_err(io)?;
            } else {
                spool.write_all(text.as_bytes()).map_err(io)?;
            }
            at += valid;
            for byte in &pending[at..at + invalid] {
                write!(spool, r"\x{byte:02X}").map_err(io)?;
            }
            at += invalid;
            if invalid == 0 {
                break;
            }
        }
        pending.drain(..at);
        Ok(())
    };
    for bytes in raw.chunks() {
        emit(&bytes.map_err(io)?, false)?;
    }
    emit(&[], true)?;
    spool.write_all(ending.as_bytes()).map_err(io)?;
    Ok(spool.finish())
}
