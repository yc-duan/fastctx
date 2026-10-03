//! Streaming line selection and character-accurate coverage notes.

use super::Range;
use crate::head_note::{CoverageTotal, CoveredRange, HeadMetric, HeadNote};

pub(super) struct TextWindow {
    raw: String,
    first: usize,
    total: Option<usize>,
    ended: bool,
}

impl TextWindow {
    pub(super) fn read(
        mut next: impl FnMut() -> Result<Option<char>, String>,
        range: Option<Range>,
        characters: usize,
        known_total: Option<usize>,
    ) -> Result<Self, String> {
        let range = range.unwrap_or(Range {
            first: 1,
            last: usize::MAX,
        });
        let mut raw = String::new();
        let mut line = 1;
        let mut count = 0;
        let mut pending = None;
        let mut ended = false;
        let mut saw_character = false;
        let mut trailing_newline = false;
        let mut total = known_total;
        loop {
            let character = match pending.take() {
                Some(character) => Some(character),
                None => next()?,
            };
            let Some(mut character) = character else {
                ended = true;
                total = Some(if !saw_character {
                    0
                } else {
                    line - usize::from(trailing_newline)
                });
                break;
            };
            if character == '\r' {
                match next()? {
                    Some('\n') => character = '\n',
                    other => pending = other,
                }
            }
            saw_character = true;
            trailing_newline = character == '\n';
            if line >= range.first {
                raw.push(character);
                count += 1;
                if count > characters {
                    break;
                }
            }
            if character == '\n' {
                if line == range.last {
                    ended = true;
                    break;
                }
                line += 1;
            }
        }
        Ok(Self {
            raw,
            first: range.first,
            total,
            ended,
        })
    }

    pub(super) fn fit(
        &self,
        subject: &str,
        bytes: u64,
        fact: Option<&str>,
        budget: usize,
    ) -> Option<(String, bool)> {
        let boundaries: Vec<_> = self
            .raw
            .char_indices()
            .map(|(offset, _)| offset)
            .chain(std::iter::once(self.raw.len()))
            .collect();
        let render = |count| self.render(subject, bytes, fact, boundaries[count]);
        let full = render(boundaries.len() - 1);
        if full.chars().count() <= budget {
            return Some((full, self.ended));
        }
        let mut low = 0;
        let mut high = boundaries.len() - 1;
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            if render(middle).chars().count() <= budget {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        // Completing a line shortens its coverage note, so note cost is not
        // monotone. Check later prefixes until the body alone cannot fit.
        let minimum_head = HeadNote::new(subject, HeadMetric::event(""))
            .render()
            .chars()
            .count()
            + 1;
        let mut body_cost = 0;
        let mut line = self.first;
        let mut at_start = true;
        for (index, character) in self.raw.chars().enumerate() {
            if at_start {
                body_cost += line.to_string().len() + 1;
            }
            body_cost += 1;
            at_start = character == '\n';
            if at_start {
                line += 1;
            }
            let cost = body_cost + usize::from(!at_start);
            if cost + minimum_head > budget {
                break;
            }
            if index + 1 > low && render(index + 1).chars().count() <= budget {
                low = index + 1;
            }
        }
        let output = render(low);
        if output.chars().count() > budget || (low == 0 && !self.raw.is_empty()) {
            return None;
        }
        Some((output, self.ended && low == boundaries.len() - 1))
    }

    fn render(&self, subject: &str, bytes: u64, fact: Option<&str>, end: usize) -> String {
        let raw = &self.raw[..end];
        let lines = crate::encoding::view::line_count(raw);
        let last = self.first + lines.saturating_sub(1);
        let cut = end < self.raw.len() || !self.ended;
        let midline = cut && !raw.is_empty() && !raw.ends_with('\n');
        let metric = if lines == 0 {
            HeadMetric::count(0, "line", "lines")
        } else if midline && lines == 1 {
            let shown = raw.chars().count();
            let tail = &self.raw[end..];
            let rest = tail
                .split_once('\n')
                .map(|(line, _)| line)
                .or_else(|| self.ended.then_some(tail));
            let total = rest
                .map(|rest| (shown + rest.chars().count()).to_string())
                .unwrap_or_else(|| format!("at least {}", shown + 1));
            HeadMetric::event(format!(
                "line {}, chars 1-{shown} of {total} shown",
                self.first
            ))
        } else {
            HeadMetric::Coverage {
                unit: "lines",
                ranges: vec![CoveredRange::new(self.first, last)],
                total: self
                    .total
                    .map(CoverageTotal::Exact)
                    .unwrap_or(CoverageTotal::FileBytes(bytes)),
            }
        };
        let mut head = HeadNote::new(subject, metric);
        if midline && lines > 1 {
            let column = raw.rsplit('\n').next().unwrap().chars().count();
            head = head.fact(format!("line {last}, chars 1-{column} shown"));
        }
        if let Some(fact) = fact {
            head = head.fact(fact);
        }
        let mut output = head.render();
        output.push('\n');
        for (index, line) in raw.split_inclusive('\n').enumerate() {
            use std::fmt::Write;
            write!(&mut output, "{}\t{line}", self.first + index).unwrap();
        }
        if !raw.is_empty() && !raw.ends_with('\n') {
            output.push('\n');
        }
        output
    }
}
