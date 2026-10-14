use crate::Content;

pub(crate) struct Output {
    text: String,
    left: usize,
    total: usize,
    stop_room: usize,
    stopped: bool,
    facts: String,
    facts_room: usize,
    open_row: Option<u32>,
    cut: Option<(usize, usize)>,
    cells: u64,
}
pub(crate) struct Header {
    offset: usize,
    length: usize,
    cost: usize,
    label: String,
}
#[derive(Default)]
pub(crate) struct Lines {
    pub total: u64,
    pub first: u64,
    pub last: u64,
    pub cut: Option<(u64, usize, usize)>,
}
impl Lines {
    pub fn facts(&self, selected: bool) -> String {
        if self.total == 0 {
            return count(0, "line");
        }
        let base = if self.first == 0 && self.total > 0 {
            format!("no lines shown of {}", self.total)
        } else if !selected && self.first <= 1 && self.last == self.total && self.cut.is_none() {
            count(self.total, "line")
        } else if self.first == self.last {
            format!("line {} of {}", self.first, self.total)
        } else {
            format!("lines {}-{} of {}", self.first, self.last, self.total)
        };
        match self.cut {
            Some((line, shown, total)) => format!("{base}; line {line}: {shown} of {total} characters shown"),
            None => base,
        }
    }
}
impl Output {
    pub fn probe(budget: usize) -> Self {
        let plain = Self::new(budget);
        // A complete output must not be rejected merely because a stopping
        // location or the longest possible facts were reserved but never used.
        let extra = plain.stop_room
            + line_reserve().len()
            + crate::spreadsheet::sheet_reserve().len()
            + crate::spreadsheet::workbook_reserve();
        Self::new(budget.saturating_add(extra))
    }
    pub fn new(budget: usize) -> Self {
        // Counter widths and Excel's grid bound the longest continuation.
        // Reserve it once so a later long line cannot erase earlier output
        // because its stopping note is longer than the previous line's.
        let n = u64::MAX;
        let line = [
            format!("the rest starts at line {n} of the master and layout text"),
            format!("the rest starts at slide {n}, line {n}"),
            format!("the rest starts at sheet {n}, merged XFD1048576:XFD1048576"),
        ]
        .iter()
        .map(|rest| notice(&format!("{n}\t{n} of {n} characters shown; remaining selected content not shown"), rest))
        .max_by_key(String::len)
        .expect("candidates");
        let cell = office_oxide::xlsx::CellRef { row: 1_048_574, col: 16_383 };
        let cells = notice(
            "from XFD1048576; remaining selected content not shown",
            &crate::spreadsheet::resume(usize::MAX - 1, &cell, None),
        );
        let stop_room = line.len().max(cells.len()).min(budget);
        Self {
            text: String::new(),
            left: budget - stop_room,
            total: budget,
            stop_room,
            stopped: false,
            facts: String::new(),
            facts_room: 0,
            open_row: None,
            cut: None,
            cells: 0,
        }
    }
    pub fn reserve_facts(&mut self, room: usize) {
        self.facts_room = room;
        self.left = self.left.saturating_sub(room);
    }
    pub fn facts(&mut self, facts: String) {
        assert!(facts.len() <= self.facts_room);
        self.facts = facts;
    }
    pub fn header(&mut self, label: &str, max_facts: &str, resume: &str) -> Option<Header> {
        self.end_row();
        let text = format!("=== {label} ({max_facts}) ===\n");
        let cost = text.len();
        if self.stopped || cost > self.left {
            self.stop("content not shown", resume);
            return None;
        }
        let header = Header { offset: self.text.len(), length: text.len(), cost, label: label.into() };
        self.text.push_str(&text);
        self.left -= cost;
        Some(header)
    }
    pub fn update_header(&mut self, header: Header, facts: &str) {
        let text = format!("=== {} ({facts}) ===\n", header.label);
        let cost = text.len();
        assert!(cost <= header.cost);
        self.text.replace_range(header.offset..header.offset + header.length, &text);
        self.left += header.cost - cost;
    }
    pub fn line(&mut self, prefix: &str, value: &str, resume: &str) -> bool {
        self.end_row();
        self.cut = None;
        if self.stopped {
            return false;
        }
        let value = visible(value);
        let count = value.chars().count();
        let cost = prefix.len() + value.len() + 1;
        if cost <= self.left {
            self.text.push_str(prefix);
            self.text.push_str(&value);
            self.text.push('\n');
            self.left -= cost;
            true
        } else {
            let mut end = self.left.saturating_sub(prefix.len() + 1).min(value.len());
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            let shown = &value[..end];
            if !shown.is_empty() {
                self.text.push_str(prefix);
                self.text.push_str(shown);
                self.text.push('\n');
                self.left -= prefix.len() + shown.len() + 1;
            }
            let shown = shown.chars().count();
            self.cut = Some((shown, count));
            self.stop(
                &format!("{prefix}{shown} of {count} characters shown; remaining selected content not shown"),
                resume,
            );
            false
        }
    }
    pub fn cell(&mut self, row: u32, address: &str, value: &str, resume: &str) -> bool {
        if self.stopped {
            return false;
        }
        let text = format!("{address} {}", visible(value).replace('\t', "\\x09"));
        let tail = format!("from {address}; remaining selected content not shown");
        // Every row pays for its newline when its first cell is accepted.
        let cost = text.len() + 1;
        if cost > self.left {
            self.stop(&tail, resume);
            return false;
        }
        if self.open_row == Some(row) {
            self.text.push('\t');
        } else {
            self.end_row();
            self.open_row = Some(row);
        }
        self.text.push_str(&text);
        self.left -= cost;
        self.cells += 1;
        true
    }
    pub fn end_row(&mut self) {
        if self.open_row.take().is_some() {
            self.text.push('\n');
        }
    }
    fn stop(&mut self, facts: &str, resume: &str) {
        if self.stopped {
            return;
        }
        self.end_row();
        let text = notice(facts, resume);
        if text.len() <= self.left + self.stop_room {
            self.left = self.left + self.stop_room - text.len();
            self.stop_room = 0;
            self.text.push_str(&text);
        } else {
            self.text.clear();
        }
        self.stopped = true;
    }
    pub fn numbered(&mut self, text: &str, range: Option<(u64, u64)>, resume: impl Fn(u64, u64) -> String) -> Lines {
        let mut stats = Lines { total: text.lines().count() as u64, ..Default::default() };
        let (first, last) = range.unwrap_or((1, stats.total));
        for (i, line) in text.lines().enumerate() {
            let n = i as u64 + 1;
            if n < first || n > last {
                continue;
            }
            let accepted = self.line(&format!("{n}\t"), line, &resume(n, stats.total));
            if accepted || self.cut.is_some_and(|(shown, _)| shown > 0) {
                if stats.first == 0 {
                    stats.first = n;
                }
                stats.last = n;
            }
            if !accepted {
                stats.cut = self.cut.map(|(shown, total)| (n, shown, total));
                break;
            }
        }
        stats
    }
    pub fn stopped(&self) -> bool {
        self.stopped
    }
    pub fn cell_count(&self) -> u64 {
        self.cells
    }
    pub fn finish(mut self) -> Content {
        self.end_row();
        if self.facts.len() + self.text.len() > self.total {
            self.facts.clear();
            self.text.clear();
            self.stopped = true;
        }
        Content { facts: self.facts, text: self.text, stopped: self.stopped, whole: false }
    }
}
fn notice(facts: &str, resume: &str) -> String {
    format!("=== stopped ({facts}; {resume}) ===\n")
}
pub(crate) fn count(number: u64, noun: &str) -> String {
    format!("{number} {noun}{}", if number == 1 { "" } else { "s" })
}
pub(crate) fn line_reserve() -> String {
    format!("lines {n}-{n} of {n}; line {n}: {n} of {n} characters shown", n = u64::MAX)
}
pub(crate) fn visible(text: &str) -> String {
    let mut shown = String::new();
    for c in text.chars() {
        if c.is_control() && c != '\t' {
            for b in c.encode_utf8(&mut [0; 4]).bytes() {
                shown.push_str(&format!("\\x{b:02X}"));
            }
        } else {
            shown.push(c);
        }
    }
    shown
}
pub(crate) fn quoted(text: &str) -> String {
    let json = serde_json::to_string(text).expect("a string always serializes");
    let mut result = String::with_capacity(json.len());
    for c in json.chars() {
        // JSON permits DEL, C1 controls and Unicode line separators. Escape
        // these too, so each record remains one line and visible() cannot
        // replace a JSON character with a non-JSON escape.
        if needs_escape(c) {
            result.push_str(&format!("\\u{:04x}", c as u32));
        } else {
            result.push(c);
        }
    }
    result
}

pub(crate) fn formula(text: &str) -> String {
    if text.chars().any(needs_escape) {
        // JSON distinguishes actual controls from literal escape-looking
        // text without introducing row or cell separators into the record.
        format!("formula {}", quoted(&format!("={text}")))
    } else {
        format!("={text}")
    }
}

fn needs_escape(c: char) -> bool {
    c.is_control() || matches!(c, '\u{2028}' | '\u{2029}')
}
