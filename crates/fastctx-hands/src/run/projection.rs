//! Attribute selected canonical text using a separate, byte-preserving provenance replay.

use super::terminal::{Lines, Part};
use fastctx_types::run::{OutputPiece, OutputWindow};
use fastctx_types::run_facts::{DisplayPiece, OutputNote, OutputSpan};
use std::collections::BTreeMap;
use std::io::{self, Read};

#[derive(Clone)]
struct Portion {
    view: u64,
    line: u64,
    from: usize,
    text: String,
    note: Option<OutputNote>,
}

/// Reusable attribution for a lazy full-output line source. Its source byte offsets
/// are absolute; callers assemble fragments into their own indexed rows and UTF-16 ranges.
pub struct CapturedNotes {
    matches: super::write_notes::Matches,
    matcher: super::write_notes::Matcher,
}

impl CapturedNotes {
    pub fn read(
        path: &std::path::Path,
        bytes: u64,
        writes: &[fastctx_types::run_facts::WriteFact],
    ) -> io::Result<Self> {
        Self::read_cancellable(path, bytes, writes, None)
    }

    pub fn read_cancellable(
        path: &std::path::Path,
        bytes: u64,
        writes: &[fastctx_types::run_facts::WriteFact],
        cancelled: Option<&std::sync::atomic::AtomicBool>,
    ) -> io::Result<Self> {
        let mut matcher = super::write_notes::Matcher::new(writes)?;
        matcher.append(path, bytes, cancelled)?;
        let matches = matcher.selected(cancelled)?;
        Ok(Self { matches, matcher })
    }

    /// Unchanged patterns index only newly captured bytes. New patterns can match
    /// earlier program output too, and therefore need an index of that prefix.
    pub fn refresh(
        &mut self,
        path: &std::path::Path,
        bytes: u64,
        writes: &[fastctx_types::run_facts::WriteFact],
    ) -> io::Result<bool> {
        self.refresh_cancellable(path, bytes, writes, None)
    }

    pub fn refresh_cancellable(
        &mut self,
        path: &std::path::Path,
        bytes: u64,
        writes: &[fastctx_types::run_facts::WriteFact],
        cancelled: Option<&std::sync::atomic::AtomicBool>,
    ) -> io::Result<bool> {
        if !self.matcher.same_notes(writes)? {
            self.matcher.extend_notes(path, bytes, writes, cancelled)?;
        }
        self.matcher.append(path, bytes, cancelled)?;
        let matches = self.matcher.selected(cancelled)?;
        let changed = matches.ranges != self.matches.ranges || matches.notes != self.matches.notes;
        self.matches = matches;
        Ok(changed)
    }

    /// The returned Line.view is an attribution tag, unrelated to view-budget regions.
    /// This replay is for display only; model output always uses Lines::new.
    pub fn lines<R: Read>(&self, input: R, raw_start: u64) -> Lines<R> {
        Lines::tagged_at(input, raw_start, self.matches.ranges.clone())
    }

    pub fn note(&self, tag: u64) -> Option<&OutputNote> {
        self.matches.notes.get(&tag)
    }

    pub fn intersects(&self, from: u64, to: u64) -> bool {
        let at = self.matches.ranges.partition_point(|(range, _)| range.end <= from);
        self.matches.ranges.get(at).is_some_and(|(range, _)| range.start < to)
    }

    pub fn matches(&self) -> Vec<fastctx_types::run_facts::OutputNoteMatch> {
        self.matches
            .ranges
            .iter()
            .map(|(range, tag)| fastctx_types::run_facts::OutputNoteMatch {
                note: self.matches.notes[tag].clone(),
                from: range.start,
                to: range.end,
            })
            .collect()
    }
}

pub fn read(window: &OutputWindow, writes: &[fastctx_types::run_facts::WriteFact]) -> io::Result<Vec<OutputSpan>> {
    Ok(read_with_matches(window, writes)?.0)
}

pub fn read_with_matches(
    window: &OutputWindow,
    writes: &[fastctx_types::run_facts::WriteFact],
) -> io::Result<(Vec<OutputSpan>, Vec<fastctx_types::run_facts::OutputNoteMatch>)> {
    let projection = CapturedNotes::read(std::path::Path::new(&window.log), window.bytes, writes)?;
    let notes = projection.matches();
    let mut ranges = BTreeMap::<u64, Vec<(usize, usize)>>::new();
    let mut columns = BTreeMap::<u64, usize>::new();
    for piece in &window.pieces {
        match piece {
            OutputPiece::Text { first_line, text } => {
                let mut line = *first_line;
                for text in text.split_inclusive('\n') {
                    let column = columns.entry(line).or_default();
                    ranges.entry(line).or_default().push((*column, *column + text.chars().count()));
                    *column += text.chars().count();
                    line += u64::from(text.ends_with('\n'));
                }
            }
            OutputPiece::PartOfLine { line, from, to, .. } => {
                ranges.entry(*line).or_default().push((*from as usize - 1, *to as usize));
            }
            _ => {}
        }
    }
    let mut selected = BTreeMap::<u64, Vec<Portion>>::new();
    {
        let input = std::fs::File::open(&window.log)?.take(window.bytes);
        let mut lines = projection.lines(input, 0);
        while let Some(line) = lines.next_line()? {
            let Some(ranges) = ranges.get(&line.number) else { continue };
            let mut text = String::new();
            for part in line.parts.iter() {
                match part? {
                    Part::Text(part) => text.push_str(&part),
                    Part::Spaces(count) => text.extend(std::iter::repeat_n(' ', count)),
                    _ => {}
                }
            }
            if line.newline {
                text.push('\n');
            }
            let end = line.offset + text.chars().count();
            for &(from, to) in ranges {
                let (from, to) = (from.max(line.offset), to.min(end));
                if from < to {
                    selected.entry(line.number).or_default().push(Portion {
                        view: line.view,
                        line: line.number,
                        from,
                        text: text.chars().skip(from - line.offset).take(to - from).collect(),
                        note: None,
                    });
                }
            }
        }
    }
    for portions in selected.values_mut() {
        for portion in portions {
            portion.note = projection.note(portion.view).cloned();
        }
    }
    let mut spans = Vec::new();
    let mut image = 0;
    let mut columns = BTreeMap::<u64, usize>::new();
    for piece in &window.pieces {
        match piece {
            OutputPiece::Text { first_line, text } => {
                let mut number = *first_line;
                for segment in text.split_inclusive('\n') {
                    let column = columns.entry(number).or_default();
                    add(&mut spans, selected.get(&number), number, *column, *column + segment.chars().count(), segment);
                    *column += segment.chars().count();
                    number += u64::from(segment.ends_with('\n'));
                }
            }
            OutputPiece::PartOfLine { line, from, to, text, .. } => {
                let mut parts = Vec::new();
                add(&mut parts, selected.get(line), *line, *from as usize - 1, *to as usize, text);
                let mut offset = *from;
                for part in &mut parts {
                    if let DisplayPiece::Text { text, .. } = &part.piece {
                        let end = offset + text.chars().count() as u64;
                        let OutputPiece::PartOfLine { of, .. } = piece else { unreachable!() };
                        part.piece = DisplayPiece::PartOfLine {
                            line: *line,
                            from: offset,
                            to: end - 1,
                            of: *of,
                            text: text.clone(),
                        };
                        offset = end;
                    }
                }
                spans.extend(parts);
            }
            OutputPiece::Image { mime, width, height, .. } => {
                spans.push(OutputSpan {
                    note: None,
                    piece: DisplayPiece::Image { image, mime: mime.clone(), width: *width, height: *height },
                });
                image += 1;
            }
            OutputPiece::Omitted { first_line, last_line } => spans.push(OutputSpan {
                note: None,
                piece: DisplayPiece::Omitted { first_line: *first_line, last_line: *last_line },
            }),
            OutputPiece::Unreadable { line, reason } => spans.push(OutputSpan {
                note: None,
                piece: DisplayPiece::Unreadable { line: *line, reason: reason.clone() },
            }),
        }
    }
    Ok((spans, notes))
}

fn add(
    spans: &mut Vec<OutputSpan>,
    portions: Option<&Vec<Portion>>,
    line: u64,
    from: usize,
    to: usize,
    original: &str,
) {
    let Some(portions) = portions else {
        spans.push(OutputSpan { note: None, piece: DisplayPiece::Text { first_line: line, text: original.into() } });
        return;
    };
    let mut at = from;
    for portion in portions {
        let start = portion.from.max(at);
        let end = (portion.from + portion.text.chars().count()).min(to);
        if start >= end {
            continue;
        }
        if start > at {
            push(spans, None, line, original.chars().skip(at - from).take(start - at).collect());
        }
        // Rendered text always comes from the canonical window. Attribution alone is projected.
        push(
            spans,
            portion.note.clone(),
            portion.line,
            original.chars().skip(start - from).take(end - start).collect(),
        );
        at = end;
    }
    if at < to {
        push(spans, None, line, original.chars().skip(at - from).collect());
    }
}

fn push(spans: &mut Vec<OutputSpan>, note: Option<OutputNote>, line: u64, text: String) {
    if let Some(OutputSpan { note: last_note, piece: DisplayPiece::Text { text: last_text, .. } }) = spans.last_mut()
        && *last_note == note
    {
        last_text.push_str(&text);
    } else {
        spans.push(OutputSpan { note, piece: DisplayPiece::Text { first_line: line, text } });
    }
}

pub fn slice(span: &OutputSpan, range: std::ops::Range<usize>) -> OutputSpan {
    let mut result = span.clone();
    match &mut result.piece {
        DisplayPiece::Text { first_line, text } => {
            *first_line += memchr::memchr_iter(b'\n', &text.as_bytes()[..range.start]).count() as u64;
            *text = text[range].to_string();
        }
        DisplayPiece::PartOfLine { from, to, text, .. } => {
            *from += text[..range.start].chars().count() as u64;
            *to = *from + text[range.clone()].chars().count() as u64 - 1;
            *text = text[range].to_string();
        }
        _ => {}
    }
    result
}

pub fn remap(regions: &[super::present::ModelSpan], kept: &[crate::answer::Kept]) -> Vec<super::present::ModelSpan> {
    let mut mapped = Vec::new();
    let mut image = 0;
    for keep in kept {
        for region in regions.iter().filter(|region| region.block == keep.source) {
            let bytes = match (&region.bytes, &keep.bytes) {
                (None, None) => None,
                (Some(source), Some(kept)) => {
                    let start = source.start.max(kept.start);
                    let end = source.end.min(kept.end);
                    if start >= end {
                        continue;
                    }
                    Some((start..end, kept.start, source.start))
                }
                _ => continue,
            };
            let (span, bytes) = match bytes {
                None => {
                    let mut span = region.span.clone();
                    if let DisplayPiece::Image { image: ordinal, .. } = &mut span.piece {
                        *ordinal = image;
                        image += 1;
                    }
                    (span, None)
                }
                Some((range, kept_start, source_start)) => {
                    let span = slice(&region.span, range.start - source_start..range.end - source_start);
                    (span, Some(range.start - kept_start..range.end - kept_start))
                }
            };
            mapped.push(super::present::ModelSpan { block: keep.output, bytes, span });
        }
    }
    mapped
}
