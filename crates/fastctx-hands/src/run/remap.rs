//! Preserve source coordinates and omissions when the complete answer is fitted.

use super::present::ModelSpan;
use crate::answer::Kept;
use fastctx_types::run_facts::{DisplayPiece, OutputNote, OutputSpan};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

struct Fragment {
    region: ModelSpan,
    column: u64,
}

#[derive(Default)]
struct Row {
    chars: u64,
    column: u64,
    cut: bool,
    retained: bool,
    note: Option<OutputNote>,
    seen: bool,
}

impl Row {
    fn attribute(&mut self, note: &Option<OutputNote>) {
        if !self.seen {
            self.note = note.clone();
            self.seen = true;
        } else if self.note != *note {
            self.note = None;
        }
    }
}

pub fn remap(regions: &[ModelSpan], kept: &[Kept]) -> Vec<ModelSpan> {
    let mut rows = BTreeMap::<u64, Row>::new();
    let mut fragments = Vec::new();
    for region in regions {
        if let DisplayPiece::Text { first_line, text } = &region.span.piece {
            let mut line = *first_line;
            let mut offset = 0;
            for segment in text.split_inclusive('\n') {
                let row = rows.entry(line).or_default();
                let column = row.column + 1;
                row.column += segment.trim_end_matches('\n').chars().count() as u64;
                row.chars = row.chars.max(row.column);
                row.attribute(&region.span.note);
                let fragment = ModelSpan {
                    block: region.block,
                    bytes: region
                        .bytes
                        .as_ref()
                        .map(|range| range.start + offset..range.start + offset + segment.len()),
                    span: OutputSpan {
                        note: region.span.note.clone(),
                        piece: DisplayPiece::Text { first_line: line, text: segment.into() },
                    },
                };
                fragments.push(Fragment { region: fragment, column });
                offset += segment.len();
                line += u64::from(segment.ends_with('\n'));
            }
        } else {
            if let DisplayPiece::PartOfLine { line, to, of, .. } = region.span.piece {
                let row = rows.entry(line).or_default();
                row.chars = row.chars.max(of);
                row.column = to;
                row.attribute(&region.span.note);
            } else if let DisplayPiece::Unreadable { line, .. } = region.span.piece {
                rows.entry(line).or_default().attribute(&region.span.note);
            }
            fragments.push(Fragment { region: region.clone(), column: 1 });
        }
    }
    for fragment in &fragments {
        let selected = selections(&fragment.region, kept);
        if let Some(line) = line_of(&fragment.region.span.piece) {
            let row = rows.entry(line).or_default();
            row.retained |= !selected.is_empty();
            let size = fragment.region.bytes.as_ref().map_or(0, Range::len);
            let shown: usize = selected.iter().map(|(_, bytes)| bytes.as_ref().map_or(0, Range::len)).sum();
            row.cut |= size != shown;
        }
    }
    let notice = (0..=kept.len()).find(|block| !kept.iter().any(|keep| keep.output == *block)).unwrap_or(0);
    let mut result = Mapped::default();
    let mut missing = BTreeSet::new();
    let mut image = 0;
    for fragment in fragments {
        let region = &fragment.region;
        let selected = selections(region, kept);
        match &region.span.piece {
            DisplayPiece::Omitted { .. } => {
                // An earlier omission remains true even if its model-facing notice was cut.
                let (block, bytes) = selected.first().cloned().unwrap_or((notice, None));
                let bytes = output_bytes(region.block, block, bytes, kept);
                result.push(ModelSpan { block, bytes, span: region.span.clone() });
            }
            piece if selected.is_empty() => {
                if let Some(line) = line_of(piece) {
                    let row = &rows[&line];
                    if !row.retained && missing.insert(line) {
                        result.push(ModelSpan {
                            block: notice,
                            bytes: None,
                            span: OutputSpan {
                                note: row.note.clone(),
                                piece: DisplayPiece::Omitted { first_line: line, last_line: line },
                            },
                        });
                    }
                }
            }
            _ => {
                for (block, bytes) in selected {
                    let mut span = match (&region.bytes, &bytes) {
                        (Some(source), Some(range)) => {
                            let mut selected = range.start - source.start..range.end - source.start;
                            if let DisplayPiece::PartOfLine { text, .. } = &region.span.piece
                                && source.len() == text.len() + 1
                            {
                                // A fitted text row can retain its newline without counting it
                                // among the line's character coordinates.
                                selected.start = selected.start.min(text.len());
                                selected.end = selected.end.min(text.len());
                            }
                            super::projection::slice(&region.span, selected)
                        }
                        _ => region.span.clone(),
                    };
                    if let DisplayPiece::Text { first_line, text } = &span.piece {
                        let row = &rows[first_line];
                        if row.cut && row.chars > 0 {
                            let source = region.bytes.as_ref().expect("text source");
                            let range = bytes.as_ref().expect("text selection");
                            let DisplayPiece::Text { text: original, .. } = &region.span.piece else { unreachable!() };
                            let from = fragment.column + original[..range.start - source.start].chars().count() as u64;
                            let text = text.trim_end_matches('\n').to_string();
                            span.piece = DisplayPiece::PartOfLine {
                                line: *first_line,
                                from,
                                to: from + text.chars().count() as u64 - 1,
                                of: row.chars,
                                text,
                            };
                        }
                    }
                    if let DisplayPiece::Image { image: ordinal, .. } = &mut span.piece {
                        *ordinal = image;
                        image += 1;
                    }
                    let bytes = output_bytes(region.block, block, bytes, kept);
                    result.push(ModelSpan { block, bytes, span });
                }
            }
        }
    }
    result.entries
}

fn output_bytes(source: usize, output: usize, bytes: Option<Range<usize>>, kept: &[Kept]) -> Option<Range<usize>> {
    bytes.map(|range| {
        let keep = kept.iter().find(|keep| keep.source == source && keep.output == output).expect("selected block");
        let start = keep.bytes.as_ref().expect("text block").start;
        range.start - start..range.end - start
    })
}

fn selections(region: &ModelSpan, kept: &[Kept]) -> Vec<(usize, Option<Range<usize>>)> {
    kept.iter()
        .filter(|keep| keep.source == region.block)
        .filter_map(|keep| match (&region.bytes, &keep.bytes) {
            (Some(source), Some(kept)) => {
                let start = source.start.max(kept.start);
                let end = source.end.min(kept.end);
                (start < end).then_some((keep.output, Some(start..end)))
            }
            (None, None) => Some((keep.output, None)),
            _ => None,
        })
        .collect()
}

fn line_of(piece: &DisplayPiece) -> Option<u64> {
    match piece {
        DisplayPiece::Text { first_line, .. } => Some(*first_line),
        DisplayPiece::PartOfLine { line, .. } | DisplayPiece::Unreadable { line, .. } => Some(*line),
        _ => None,
    }
}

#[derive(Default)]
struct Mapped {
    entries: Vec<ModelSpan>,
    next_line: Option<u64>,
}

impl Mapped {
    fn push(&mut self, region: ModelSpan) {
        let next_line = match &region.span.piece {
            DisplayPiece::Text { first_line, text } => {
                Some(*first_line + memchr::memchr_iter(b'\n', text.as_bytes()).count() as u64)
            }
            _ => None,
        };
        if let DisplayPiece::Omitted { first_line, last_line } = region.span.piece
            && let Some(previous) = self.entries.last_mut()
            && previous.span.note == region.span.note
            && let DisplayPiece::Omitted { last_line: last, .. } = &mut previous.span.piece
            && *last + 1 == first_line
        {
            *last = last_line;
            previous.bytes = None;
        } else if let DisplayPiece::Text { first_line, ref text } = region.span.piece
            && self.next_line == Some(first_line)
            && let Some(previous) = self.entries.last_mut()
            && previous.block == region.block
            && previous.span.note == region.span.note
            && previous
                .bytes
                .as_ref()
                .zip(region.bytes.as_ref())
                .is_some_and(|(before, after)| before.end == after.start)
            && let DisplayPiece::Text { text: open, .. } = &mut previous.span.piece
        {
            open.push_str(text);
            previous.bytes.as_mut().expect("text range").end = region.bytes.expect("text range").end;
        } else {
            self.entries.push(region);
        }
        self.next_line = next_line;
    }
}
