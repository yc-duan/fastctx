use std::io::{self, BufRead, BufReader, Read};
use std::ops::Range;

use super::evidence::printable;
use super::file::{Plan, describe_error, hex};
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
            decode(&raw, offset, plan, end)
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
        offset += raw.len();
        consume(line)?;
    }
    Ok(())
}

fn decode(raw: &[u8], offset: usize, plan: &Plan, end: bool) -> Line {
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
            codec: Some(TextEncoding::utf8()),
            basis: "UTF-8 by convention, not proof of origin",
            issue: None,
        };
    }
    let mut chosen: Option<(TextEncoding, String)> = None;
    let mut names = Vec::new();
    let mut interpretations = Vec::new();
    let preview_budget = crate::run::window::BUDGET / (plan.choices.len() + 1);
    let mut different = false;
    for &codec in &plan.choices {
        if let Ok(text) = codec.decode_at(raw, offset)
            && (plan.ordered || text.chars().all(printable))
        {
            if codec.is_stateful() && !isolated_state(raw, end) {
                return state_boundary(raw, offset);
            }
            names.push(codec.label());
            let content = crate::text::line_content(&text);
            let total = crate::text::visible_length(content);
            let kept: String = content.chars().take(preview_budget).collect();
            let prefix: String = crate::text::visible(&kept).chars().take(preview_budget).collect();
            let note = if total > preview_budget {
                format!(" ({} of {total} characters shown)", prefix.chars().count())
            } else {
                String::new()
            };
            interpretations.push(format!("{}: {prefix:?}{note}", codec.label()));
            if let Some((prior_codec, prior)) = &chosen {
                different |= *prior != text;
                if !different && codec.label() < prior_codec.label() {
                    chosen = Some((codec, text));
                }
            } else {
                chosen = Some((codec, text));
            }
            if plan.ordered {
                break;
            }
        }
    }
    if !different && let Some((codec, text)) = chosen {
        return Line {
            bytes: offset..offset + raw.len(),
            text,
            codec: Some(codec.without_bom()),
            basis: if plan.ordered { plan.basis } else { "supported candidates agree on the text" },
            issue: None,
        };
    }
    let invalid = std::str::from_utf8(raw).err().map_or(0, |e| e.valid_up_to());
    let reason = if different {
        format!(
            "Different text interpretations at byte {} (hex {}); candidates: {}. Select the known encoding with fastctx view --encoding <label> -- <path>",
            offset + invalid,
            hex(&raw[invalid..invalid + 1]),
            format!("{}; interpretations: {}", names.join(", "), interpretations.join("; "))
        )
    } else {
        let labels: Vec<_> = plan.choices.iter().map(|c| c.label()).collect();
        format!(
            "No complete line decoding at byte {} (hex {}) using {}; this may be mixed or damaged text. Inspect the original bytes with xxd or select the known encoding with fastctx view --encoding <label> -- <path>",
            offset + invalid,
            hex(&raw[invalid..invalid + 1]),
            labels.join(", ")
        )
    };
    readonly(raw, offset, reason)
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
