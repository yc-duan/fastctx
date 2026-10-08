use std::io::{self, BufRead, BufReader, Read};
use std::ops::Range;

use super::file::{Plan, describe_error, hex};
use super::statistics::{BASIS, Statistics};
use super::{TextDecoder, TextEncoding};

pub(crate) struct Line {
    pub bytes: Range<usize>,
    pub text: String,
    pub codec: Option<TextEncoding>,
    pub basis: &'static str,
    pub issue: Option<String>,
}

pub(crate) fn visit(
    reader: impl Read,
    plan: &Plan,
    mut consume: impl FnMut(Line) -> Result<(), String>,
) -> Result<(), String> {
    let mut reader = BufReader::new(reader);
    let bom_len = plan.signature().map_or(0, |c| c.bom_len());
    let mut bom = vec![0; bom_len];
    reader.read_exact(&mut bom).map_err(|e| e.to_string())?;
    let mut offset = bom_len;
    let unit = plan.fixed.map_or(1, |c| c.unit());
    let newline = plan.fixed.map(|c| c.encode("\n").expect("newline is representable")).unwrap_or_else(|| vec![b'\n']);
    let mut previous = plan.fixed.or_else(|| plan.legacy.first().copied()).unwrap_or_else(TextEncoding::utf8);
    let mut state_error = None;
    let mut stateful = plan.fixed.filter(|c| c.is_stateful()).map(|c| TextDecoder::new(c.without_bom()));
    loop {
        let mut raw = Vec::new();
        if unit == 1 {
            reader.read_until(b'\n', &mut raw).map_err(|e| e.to_string())?;
        } else {
            loop {
                let start = raw.len();
                let mut one = [0; 4];
                let mut filled = 0;
                while filled < unit {
                    match reader.read(&mut one[filled..unit]) {
                        Ok(0) => break,
                        Ok(n) => filled += n,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(e) => return Err(e.to_string()),
                    }
                }
                raw.extend_from_slice(&one[..filled]);
                if filled < unit || raw[start..] == newline {
                    break;
                }
            }
        }
        if raw.is_empty() {
            break;
        }
        let end = reader.fill_buf().map_err(|e| e.to_string())?.is_empty();
        let mut line = if let Some(decoder) = stateful.as_mut() {
            match decoder.feed(&raw, end) {
                Ok(text) => {
                    Line { bytes: offset..offset + raw.len(), text, codec: plan.fixed, basis: plan.basis, issue: None }
                }
                Err(error) => {
                    let reason = state_error.get_or_insert_with(|| {
                        let local = match error {
                            super::EncodingError::Malformed { encoding, offset: failed } => {
                                super::EncodingError::Malformed { encoding, offset: failed.saturating_sub(offset) }
                            }
                            error => error,
                        };
                        describe_error(&local, &raw, offset)
                    });
                    readonly(&raw, offset, reason.clone())
                }
            }
        } else {
            decode(&raw, offset, plan, end, previous)
        };
        if line.issue.is_some() {
            let crlf =
                plan.fixed.map(|codec| codec.encode("\r\n").expect("line ending")).unwrap_or_else(|| b"\r\n".to_vec());
            let ending = if raw.ends_with(&crlf) {
                "\r\n"
            } else if raw.ends_with(&newline) {
                "\n"
            } else {
                ""
            };
            let removed = if ending == "\r\n" {
                crlf.len()
            } else if ending == "\n" {
                newline.len()
            } else {
                0
            };
            line.text = preview(&raw[..raw.len() - removed]).replace('\r', r"\x0D").replace('\n', r"\x0A");
            line.text.push_str(ending);
        }
        if let Some(codec) = line.codec {
            previous = codec;
        }
        offset += raw.len();
        consume(line)?;
    }
    Ok(())
}

fn decode(raw: &[u8], offset: usize, plan: &Plan, end: bool, previous: TextEncoding) -> Line {
    if let Some(codec) = plan.fixed {
        return match codec.decode_at(raw, offset) {
            Ok(text) => Line {
                bytes: offset..offset + raw.len(),
                text,
                codec: Some(codec.without_bom()),
                basis: plan.basis,
                issue: None,
            },
            Err(error) => readonly(raw, offset, error),
        };
    }
    if !plan.ordered && super::iso2022::has_switch(raw) {
        let codec = super::explicit("iso-2022-jp", &[]).expect("known codec");
        return match codec.decode_at(raw, offset) {
            Ok(text) if isolated_state(raw, end) => Line {
                bytes: offset..offset + raw.len(),
                text,
                codec: Some(codec),
                basis: "ISO-2022-JP escapes and strict line validation",
                issue: None,
            },
            Ok(_) => state_boundary(raw, offset),
            Err(error) => readonly(raw, offset, error),
        };
    }
    if !plan.ordered
        && let Ok(text) = std::str::from_utf8(raw)
    {
        return Line {
            bytes: offset..offset + raw.len(),
            text: text.into(),
            codec: Some(if raw.is_ascii() && !previous.is_stateful() { previous } else { TextEncoding::utf8() }),
            basis: if raw.is_ascii() && previous != TextEncoding::utf8() { BASIS } else { "UTF-8" },
            issue: None,
        };
    }
    if plan.ordered {
        for &codec in &plan.choices {
            if let Ok(text) = codec.decode_at(raw, offset) {
                if codec.is_stateful() && !isolated_state(raw, end) {
                    return state_boundary(raw, offset);
                }
                return decoded(raw, offset, codec, text, plan.basis);
            }
        }
    }
    let statistics = Statistics::for_line(raw);
    if !plan.ordered {
        // A positive local score permits short lines to use the pooled legacy
        // evidence. A locally implausible codec cannot overrule a different line.
        let context =
            plan.legacy.iter().copied().filter(|codec| statistics.score(*codec).is_some_and(|score| score > 0));
        let context = select(raw, offset, context);
        let local = select(raw, offset, statistics.leaders().into_iter());
        let selected = match (context, local) {
            (Some(context), Some(local)) if context.0 != local.0 && stable(raw, local.0) => Some(local),
            (Some(context), _) => Some(context),
            (None, local) => local,
        };
        if let Some((codec, text)) = selected {
            return decoded(raw, offset, codec, text, BASIS);
        }
    }
    let mut alternatives = Vec::new();
    let mut readings = Vec::new();
    if !plan.ordered {
        for (codec, _) in statistics.ranked() {
            if let Ok(text) = codec.decode_at(raw, offset)
                && !readings.contains(&text)
            {
                alternatives.push(codec);
                readings.push(text);
                if alternatives.len() == 3 {
                    break;
                }
            }
        }
    }
    let focus = readings.first().map_or(0, |first| {
        readings
            .iter()
            .skip(1)
            .map(|other| first.chars().zip(other.chars()).take_while(|(a, b)| a == b).count())
            .min()
            .unwrap_or(0)
    });
    let alternatives: Vec<_> = alternatives
        .iter()
        .zip(&readings)
        .map(|(codec, text)| format!("{} {:?}", codec.label(), excerpt_at(crate::text::line_content(text), focus)))
        .collect();
    let invalid = std::str::from_utf8(raw).err().map_or(0, |e| e.valid_up_to());
    let reason = if !alternatives.is_empty() {
        format!(
            "its encoding is undecided, it reads as {}; view the file with --encoding and one of these to read it as that",
            alternatives.join(", ")
        )
    } else {
        let labels = if plan.ordered {
            plan.choices.iter().map(|c| c.label()).collect::<Vec<_>>().join(", ")
        } else {
            "UTF-8 or any older encoding".into()
        };
        format!(
            "it does not read whole as {labels}, from byte {} (hex {}), so it may be damaged; xxd shows the bytes, or view the file with --encoding to name its encoding",
            offset + invalid,
            hex(&raw[invalid..invalid + 1])
        )
    };
    readonly(raw, offset, reason)
}

fn stable(raw: &[u8], codec: TextEncoding) -> bool {
    let Some(first) = raw.iter().position(|byte| !byte.is_ascii()) else {
        return false;
    };
    let last = raw.iter().rposition(|byte| !byte.is_ascii()).expect("non-ASCII input");
    // Test whether independent halves of the non-ASCII span support the same
    // reading. This distinguishes a real encoding switch from a short heading
    // whose winner changes with a few bytes; no score-margin threshold is used.
    let middle = first + (last + 1 - first) / 2;
    let Some(split) = (middle..=last)
        .find(|&split| split > first && codec.decode(&raw[..split]).is_ok() && codec.decode(&raw[split..]).is_ok())
    else {
        return false;
    };
    [&raw[..split], &raw[split..]].into_iter().all(|part| {
        let statistics = Statistics::for_line(part);
        select(part, 0, statistics.leaders().into_iter()).is_some_and(|(candidate, _)| candidate == codec)
    })
}

fn select(raw: &[u8], offset: usize, codecs: impl Iterator<Item = TextEncoding>) -> Option<(TextEncoding, String)> {
    let mut selected: Option<(TextEncoding, String)> = None;
    for codec in codecs {
        if let Ok(text) = codec.decode_at(raw, offset) {
            if let Some((_, previous)) = &selected {
                if previous != &text {
                    return None;
                }
            } else {
                selected = Some((codec, text));
            }
        }
    }
    selected
}

fn decoded(raw: &[u8], offset: usize, codec: TextEncoding, text: String, basis: &'static str) -> Line {
    Line { bytes: offset..offset + raw.len(), text, codec: Some(codec.without_bom()), basis, issue: None }
}

/// Diagnostic previews show up to 80 rendered characters around the evidence.
/// Omission counts make shortening explicit; the source text stays whole.
pub(crate) fn excerpt(text: &str) -> String {
    let focus = text.find(r"\x").map_or(0, |at| text[..at].chars().count());
    excerpt_at(text, focus)
}

fn excerpt_at(text: &str, focus: usize) -> String {
    const WIDTH: usize = 80;
    let length = crate::text::visible_length(text);
    if length <= WIDTH {
        return crate::text::visible(text);
    }
    let start = focus.saturating_sub(WIDTH / 2);
    let start = text.char_indices().nth(start).map_or(text.len(), |(byte, _)| byte);
    let before = crate::text::visible_length(&text[..start]);
    let kept: String = text[start..].chars().take(WIDTH).collect();
    let mut shown: String = crate::text::visible(&kept).chars().take(WIDTH).collect();
    let after = length - before - shown.chars().count();
    if before > 0 {
        shown = format!("({before} characters before)… {shown}");
    }
    if after > 0 {
        shown.push_str(&format!("… ({after} more characters)"));
    }
    shown
}

fn readonly(raw: &[u8], offset: usize, reason: String) -> Line {
    Line {
        bytes: offset..offset + raw.len(),
        text: preview(raw),
        codec: None,
        basis: "read-only byte preview",
        issue: Some(reason),
    }
}

/// Retains valid UTF-8 runs and spells undecodable bytes without replacement characters.
fn preview(mut bytes: &[u8]) -> String {
    let mut text = String::new();
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(valid) => {
                text.push_str(valid);
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                text.push_str(std::str::from_utf8(&bytes[..valid]).expect("validated prefix"));
                let length = error.error_len().unwrap_or(bytes.len() - valid);
                for byte in &bytes[valid..valid + length] {
                    text.push_str(&format!(r"\x{byte:02X}"));
                }
                bytes = &bytes[valid + length..];
            }
        }
    }
    text
}

fn isolated_state(raw: &[u8], end: bool) -> bool {
    end || super::iso2022::State::Ascii.after(raw) == super::iso2022::State::Ascii
}

fn state_boundary(raw: &[u8], offset: usize) -> Line {
    readonly(
        raw,
        offset,
        format!(
            "ISO-2022-JP leaves a non-ASCII state at the line boundary after byte {} (last hex {}); the next segment's state is unresolved. Select ISO-2022-JP alone if the whole file uses it, or inspect the boundary bytes with xxd",
            offset + raw.len(),
            hex(&raw[raw.len() - 1..])
        ),
    )
}
