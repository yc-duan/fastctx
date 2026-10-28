//! Answer-sized samples collected beside the existing decoding stream.

use std::collections::VecDeque;
use std::path::Path;

use super::outline::Outline;
use super::text::{Read, Reader};
use super::{Options, Section, head};

/// Bytes a file may take while its content is not known yet: shown whole
/// within this, sampled beyond it; several files share it (`world-view.md`,
/// 探索).
pub(super) const WHOLE: usize = 10_000 * 4;
/// The start of a file with an outline: its first lines, which say what it
/// is for; the outline says where the rest is.
pub(super) const BRIEF: usize = 1_000 * 4;
pub(super) const BRIEF_LINES: u64 = 20;
/// The start and the end of a file with no outline, its only map.
pub(super) const HEAD: usize = 2_000 * 4;
pub(super) const TAIL: usize = 1_000 * 4;
pub(super) const ENTRIES: usize = 40;

/// How much of a file a sample may take.
#[derive(Clone, Copy, Debug)]
pub(super) struct Limits {
    /// The start of a file with no outline, and its end.
    pub head: usize,
    pub tail: usize,
    /// The start of a file with an outline.
    pub brief: usize,
    /// The whole sample.
    pub room: usize,
    /// A sample even of a file that fits whole: one of several files that
    /// share the window.
    pub sampled: bool,
}

impl Limits {
    /// One file explored alone.
    pub fn alone() -> Self {
        Self { head: HEAD, tail: TAIL, brief: BRIEF, room: WHOLE, sampled: false }
    }

    /// One of several files, each with `share` bytes of the window.
    pub fn share(share: usize) -> Self {
        Self {
            head: HEAD.min(share / 2),
            tail: TAIL.min(share / 4),
            brief: BRIEF.min(share / 2),
            room: share,
            sampled: true,
        }
    }
}

#[derive(Clone)]
pub(super) struct Entry {
    pub number: u64,
    pub from: u64,
    pub length: u64,
    pub text: String,
    pub note: String,
}

impl Entry {
    pub fn cost(&self) -> usize {
        let location = if self.from > 1 || self.text.chars().count() as u64 != self.length {
            format!("=== line {} (characters {}-{} of {} shown) ===\n", self.number, self.from, self.end(), self.length)
                .len()
        } else {
            0
        };
        self.number.to_string().len() + 2 + self.text.len() + self.note.len() + location
    }

    pub fn end(&self) -> u64 {
        self.from + self.text.chars().count() as u64 - 1
    }

    pub fn write(&self, out: &mut String) {
        out.push_str(&self.note);
        if self.from > 1 || self.text.chars().count() as u64 != self.length {
            out.push_str(&format!(
                "=== line {} (characters {}-{} of {} shown) ===\n",
                self.number,
                self.from,
                self.end(),
                self.length
            ));
        }
        out.push_str(&format!("{}\t{}\n", self.number, self.text));
    }

    /// An outline's entry says only where it is: inside a long line, as
    /// `line:character` the way a compiler points, the text from there.
    fn located(&self) -> String {
        if self.from > 1 { format!("{}:{}", self.number, self.from) } else { self.number.to_string() }
    }

    pub fn outline_cost(&self) -> usize {
        self.located().len() + 2 + self.text.len() + self.note.len()
    }

    pub fn write_outline(&self, out: &mut String) {
        out.push_str(&self.note);
        out.push_str(&format!("{}\t{}\n", self.located(), self.text));
    }
}

/// A deterministic, evenly spaced sample with bounded retained text.
pub(super) struct Sample {
    pub total: u64,
    stride: u64,
    entries: Vec<(u64, Entry)>,
    last: Option<Entry>,
}

impl Sample {
    pub fn new() -> Self {
        Self { total: 0, stride: 1, entries: Vec::new(), last: None }
    }

    pub fn push(&mut self, entry: Entry) {
        let index = self.total;
        self.total += 1;
        self.last = Some(entry.clone());
        if index.is_multiple_of(self.stride) {
            self.entries.push((index, entry));
        }
        if self.entries.len() > ENTRIES * 2 {
            self.stride *= 2;
            self.entries.retain(|(index, _)| index.is_multiple_of(self.stride));
        }
    }

    pub fn complete_line(&mut self, line: &Entry) {
        for (_, entry) in &mut self.entries {
            if entry.number == line.number {
                if !line.text.is_empty() {
                    *entry = line.clone();
                } else {
                    entry.length = line.length;
                }
            }
        }
        if let Some(entry) = &mut self.last
            && entry.number == line.number
        {
            if !line.text.is_empty() {
                *entry = line.clone();
            } else {
                entry.length = line.length;
            }
        }
    }

    pub fn finish(mut self, budget: usize, cap: usize) -> (u64, Vec<Entry>) {
        if let Some(last) = self.last
            && self.entries.last().is_none_or(|(index, _)| *index != self.total - 1)
        {
            self.entries.push((self.total - 1, last));
        }
        let entries: Vec<_> = self
            .entries
            .into_iter()
            .filter_map(|(_, entry)| {
                let cost = entry.outline_cost();
                (!entry.text.is_empty() && cost <= budget).then_some((entry, cost))
            })
            .collect();
        let at = |i: usize, count: usize| if count <= 1 { 0 } else { i * (entries.len() - 1) / (count - 1) };
        let mut count = entries.len().min(cap);
        // Choose the count before spacing entries so a tight budget still covers the outline.
        while count > 0 && (0..count).map(|i| entries[at(i, count)].1).sum::<usize>() > budget {
            count -= 1;
        }
        let selected = (0..count).map(|i| entries[at(i, count)].0.clone()).collect();
        (self.total, selected)
    }
}

pub(super) struct Exploring {
    limits: Limits,
    head: Reader,
    /// The first lines, shown instead of `head` when the file has an outline.
    brief: Reader,
    brief_length: Option<u64>,
    outline: Outline,
    tail: VecDeque<Entry>,
    tail_cost: usize,
    tail_location: usize,
    line: Entry,
    prefix: String,
    pending_cr: bool,
    first_time: Option<String>,
    last_time: Option<String>,
    previous_note: String,
    capture: bool,
    last_line: bool,
    total_cost: usize,
    line_cost: usize,
    codec_started: bool,
    head_length: Option<u64>,
}

impl Exploring {
    pub fn new(path: &Path, limits: Limits) -> Self {
        Self {
            limits,
            head: Reader::preview(limits.head),
            brief: Reader::preview_lines(limits.brief, BRIEF_LINES),
            brief_length: None,
            outline: Outline::new(path),
            tail: VecDeque::new(),
            tail_cost: 0,
            tail_location: format!("=== line {n} (characters {n}-{n} of {n} shown) ===\n", n = u64::MAX).len(),
            line: Entry { number: 1, from: 1, length: 0, text: String::new(), note: String::new() },
            prefix: String::new(),
            pending_cr: false,
            first_time: None,
            last_time: None,
            previous_note: "[from here: UTF-8]".into(),
            capture: true,
            last_line: false,
            total_cost: 0,
            line_cost: 0,
            codec_started: false,
            head_length: None,
        }
    }

    pub fn begin_line(&mut self, last: bool) {
        self.capture = self.line.number == 1 || last || self.outline.structured();
        self.last_line = last;
    }

    pub fn note(&mut self, note: &str) {
        let bytes = note.starts_with("[the next line");
        let first_codec = !bytes && !self.codec_started;
        if !bytes {
            self.codec_started = true;
        }
        if bytes || !first_codec && self.previous_note != note {
            if !self.head.prefix_filled() {
                self.head.note(note);
            }
            if self.line.number <= BRIEF_LINES && !self.brief.prefix_filled() {
                self.brief.note(note);
            }
            self.line_cost += note.len() + 1;
        }
        if !bytes {
            self.previous_note = note.into();
        }
        self.line.note = format!("{note}\n");
    }

    pub fn take(&mut self, text: &str) {
        if !self.head.prefix_filled() {
            self.head.take(text);
        }
        if self.line.number <= BRIEF_LINES && !self.brief.prefix_filled() {
            self.brief.take(text);
        }
        let mut text = text;
        if self.pending_cr {
            if let Some(rest) = text.strip_prefix('\n') {
                self.end_line();
                text = rest;
            } else if !text.is_empty() {
                self.extend("\r");
                self.pending_cr = false;
            } else {
                return;
            }
        }
        for part in text.split_inclusive('\n') {
            if let Some(content) = part.strip_suffix('\n') {
                self.extend(content.strip_suffix('\r').unwrap_or(content));
                self.end_line();
            } else if let Some(content) = part.strip_suffix('\r') {
                self.extend(content);
                self.pending_cr = true;
            } else {
                self.extend(part);
            }
        }
    }

    fn extend(&mut self, text: &str) {
        self.outline.chunk(text, self.line.number, self.line.length);
        let plain = text.is_ascii() && !text.bytes().any(|b| b.is_ascii_control() && b != b'\t');
        let shown =
            if plain { std::borrow::Cow::Borrowed(text) } else { std::borrow::Cow::Owned(super::text::visible(text)) };
        self.line.length += if plain { text.len() as u64 } else { shown.chars().count() as u64 };
        if self.capture {
            let room = WHOLE.saturating_sub(self.prefix.len());
            self.prefix.push_str(crate::answer::prefix(text, room));
        }
        self.line.text.push_str(&shown);
        self.line_cost = self.line_cost.saturating_add(shown.len()).min(WHOLE + 1);
        if self.line.text.len() > self.limits.tail {
            let mut at = self.line.text.len() - self.limits.tail;
            while !self.line.text.is_char_boundary(at) {
                at += 1;
            }
            self.line.from += if plain && self.line.text[..at].is_ascii() {
                at as u64
            } else {
                self.line.text[..at].chars().count() as u64
            };
            self.line.text.drain(..at);
        }
    }

    fn end_line(&mut self) {
        if self.head.cut_line() == Some(self.line.number) {
            self.head_length = Some(self.line.length);
        }
        if self.brief.cut_line() == Some(self.line.number) {
            self.brief_length = Some(self.line.length);
        }
        self.total_cost =
            self.total_cost.saturating_add(self.line_cost + self.line.number.to_string().len() + 2).min(WHOLE + 1);
        self.line_cost = 0;
        if self.line.number == 1 {
            self.first_time = super::outline::timestamp(&self.prefix);
        }
        if self.last_line {
            self.last_time = super::outline::timestamp(&self.prefix);
        }
        self.outline.line(&self.prefix, &self.line);
        self.tail_cost += self.line.cost();
        let next = self.line.number + 1;
        self.tail.push_back(std::mem::replace(
            &mut self.line,
            Entry { number: next, from: 1, length: 0, text: String::new(), note: String::new() },
        ));
        while self.tail_cost > self.limits.tail && self.tail.len() > 1 {
            let cut = self.tail_cost - self.limits.tail + self.tail_location;
            if self.tail.front().unwrap().text.len() > cut {
                let first = self.tail.front_mut().unwrap();
                let before = first.cost();
                let at = crate::answer::prefix(&first.text, cut).len();
                first.from += first.text[..at].chars().count() as u64;
                first.text.drain(..at);
                self.tail_cost = self.tail_cost - before + first.cost();
                continue;
            }
            let mut previous = self.tail.pop_front().unwrap();
            self.tail_cost -= previous.cost();
            if previous.text.capacity() > self.line.text.capacity() {
                previous.text.clear();
                self.line.text = previous.text;
            }
        }
        self.prefix.clear();
        self.pending_cr = false;
    }

    pub fn fits(&self, path: &Path, options: &Options, facts: &str, total: u64) -> bool {
        // One of several files is sampled however small it is, unless its
        // sample would be all of it: it is empty, or one line its start
        // holds whole.
        if self.limits.sampled {
            return total == 0 || total == 1 && self.head.cut_line().is_none();
        }
        let pending = if self.line.length > 0 || self.pending_cr {
            self.line_cost + usize::from(self.pending_cr) * 4 + self.line.number.to_string().len() + 2
        } else {
            0
        };
        let measure = if total == 0 { "empty".into() } else { super::plural(total, "line", "lines") };
        self.total_cost + pending + head(path, options, &format!("{facts}{measure}")).len() < WHOLE
    }

    pub fn finish(mut self, path: &Path, options: &Options, budget: usize, full: &Read, facts: &str) -> Section {
        if self.pending_cr {
            self.extend("\r");
        }
        if self.line.length > 0 {
            self.end_line();
        }
        let total = full.total_lines;
        // A file with an outline is read by it: its first lines say what it
        // is for, the outline where the rest is. One long line has no lines
        // to point at, and keeps its start and end.
        if total > 1 && (self.outline.found() > 0 || self.outline.csv_rows().is_some()) {
            return self.finish_outlined(path, options, budget, full, facts);
        }
        let mut head_read = self.head.finish();
        if let (Some(cut), Some(length)) = (&mut head_read.cut, self.head_length) {
            cut.length = length;
        }
        let first_tail = self.tail.front().expect("a large file has lines");
        let head_last = head_read.cut.as_ref().map_or(head_read.shown_lines, |cut| cut.line);
        let mut measure = if total == 1 {
            let cut = head_read.cut.as_ref().expect("large single line");
            format!(
                "line 1: characters 1-{} and {}-{} of {} shown",
                cut.to, first_tail.from, first_tail.length, first_tail.length
            )
        } else if first_tail.number <= head_last {
            // The beginning and the end meet inside the last line, which
            // the facts after this and the end's own heading cut apart.
            format!("lines 1-{total} of {total}")
        } else {
            format!("lines 1-{head_last} and {}-{total} of {total}", first_tail.number)
        };
        if let Some(cut) = &head_read.cut
            && total > 1
        {
            measure.push_str(&format!("; {}", super::text::cut_facts(cut)));
        }
        let mut text = format!("{}\n{}", head(path, options, &format!("{facts}{measure}")), head_read.body);
        if let Some(cut) = &head_read.cut {
            let last = if first_tail.number == cut.line { first_tail.from - 1 } else { cut.length };
            let later = if first_tail.number > cut.line + 1 {
                format!(", and lines {}-{}", cut.line + 1, first_tail.number - 1)
            } else {
                String::new()
            };
            text.push_str(&format!("=== line {}: characters {}-{last}{later} not shown ===\n", cut.line, cut.to + 1));
        } else if head_last + 1 < first_tail.number {
            text.push_str(&format!("=== lines {}-{} not shown ===\n", head_last + 1, first_tail.number - 1));
        }
        if first_tail.from > 1 && head_read.cut.as_ref().is_none_or(|cut| cut.line != first_tail.number) {
            text.push_str(&format!(
                "=== line {}: characters 1-{} not shown ===\n",
                first_tail.number,
                first_tail.from - 1
            ));
        }
        let mut previous_note = String::new();
        for mut entry in self.tail {
            if entry.note.starts_with("[from here:") && !facts.contains("mixed encodings") {
                entry.note.clear();
            }
            if entry.note == previous_note {
                entry.note.clear();
            } else {
                previous_note = entry.note.clone();
            }
            entry.write(&mut text);
        }
        let times = (self.first_time.take(), self.last_time.take());
        Self::close(self.limits, self.outline, text, times, path, options, budget, full, facts)
    }

    /// The first lines of a file with an outline, then the outline.
    fn finish_outlined(self, path: &Path, options: &Options, budget: usize, full: &Read, facts: &str) -> Section {
        let total = full.total_lines;
        let mut read = self.brief.finish();
        if let (Some(cut), Some(length)) = (&mut read.cut, self.brief_length) {
            cut.length = length;
        }
        let last = read.cut.as_ref().map_or(read.shown_lines, |cut| cut.line);
        let mut measure = format!("lines 1-{last} of {total}");
        if let Some(cut) = &read.cut {
            measure.push_str(&format!("; {}", super::text::cut_facts(cut)));
        }
        let mut text = format!("{}\n{}", head(path, options, &format!("{facts}{measure}")), read.body);
        if let Some(cut) = &read.cut
            && cut.to < cut.length
        {
            text.push_str(&format!("=== line {}: characters {}-{} not shown ===\n", cut.line, cut.to + 1, cut.length));
        }
        if last < total {
            text.push_str(&format!("=== lines {}-{total} not shown ===\n", last + 1));
        }
        Self::close(self.limits, self.outline, text, (None, None), path, options, budget, full, facts)
    }

    /// Closes a sample with the outline, or a log's first and last times,
    /// and a CSV's count of rows, as far as the room left allows.
    #[allow(clippy::too_many_arguments)]
    fn close(
        limits: Limits,
        outline: Outline,
        mut text: String,
        times: (Option<String>, Option<String>),
        path: &Path,
        options: &Options,
        budget: usize,
        full: &Read,
        facts: &str,
    ) -> Section {
        let total = full.total_lines;
        let outline_room = limits.room.saturating_sub(text.len()).min(budget.saturating_sub(text.len()));
        let csv_rows = outline.csv_rows();
        let reserve = format!(
            "=== outline ({n} of {n} entries shown) ===\n=== CSV ({n} rows including the header) ===\n",
            n = u64::MAX
        )
        .len();
        let (count, entries) = outline.finish(outline_room.saturating_sub(reserve), ENTRIES);
        if count > 0 {
            text.push_str(&format!("=== outline ({} of {count} entries shown) ===\n", entries.len()));
            let mut previous_note = String::new();
            for mut entry in entries {
                if entry.note.starts_with("[from here:") && !facts.contains("mixed encodings") {
                    entry.note.clear();
                }
                if entry.note == previous_note {
                    entry.note.clear();
                } else {
                    previous_note = entry.note.clone();
                }
                entry.write_outline(&mut text);
            }
        } else if let (Some(first), Some(last)) = times {
            text.push_str(&format!("=== timestamps (line 1: {first}; line {total}: {last}) ===\n"));
        }
        if let Some(rows) = csv_rows {
            text.push_str(&format!("=== CSV ({rows} rows including the header) ===\n"));
        }
        let mut section =
            Section { cost: text.len(), text, shown: true, partial: false, failed: false, whole: false, pictures: Vec::new() };
        if section.cost > budget {
            // The shared answer budget is distinct from the exploration window.
            let mut limited = options.clone();
            limited.lines = Some((1, total));
            section = super::text::section(path, &limited, full, facts);
        }
        section
    }
}
