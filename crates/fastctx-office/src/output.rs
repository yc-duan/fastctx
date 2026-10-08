use crate::Content;

pub(crate) struct Output {
    text: String,
    left: usize,
    stopped: bool,
}

impl Output {
    pub fn new(budget: usize) -> Self {
        Self { text: String::new(), left: budget, stopped: false }
    }

    /// Keep space for a location and a continuation even when a single
    /// source line exceeds the entire budget. Counts have at most 20 digits.
    pub fn line(&mut self, prefix: &str, value: &str, resume: &str) -> bool {
        if self.stopped {
            return false;
        }
        let value = visible(value);
        let count = value.chars().count();
        let notice = format!(
            "=== stopped ({prefix}characters 1-{} of {count} shown; remaining selected content not shown; {resume}) ===\n",
            u64::MAX
        );
        let reserve = notice.chars().count();
        let cost = prefix.chars().count() + count + 1;
        if cost + reserve <= self.left {
            self.text.push_str(prefix);
            self.text.push_str(&value);
            self.text.push('\n');
            self.left -= cost;
            true
        } else {
            let room = self.left.saturating_sub(reserve + prefix.chars().count() + 1);
            if room > 0 {
                self.text.push_str(prefix);
                self.text.extend(value.chars().take(room));
                self.text.push('\n');
                self.left -= prefix.chars().count() + room + 1;
            }
            let notice = format!(
                "=== stopped ({prefix}characters 1-{room} of {count} shown; remaining selected content not shown; {resume}) ===\n"
            );
            if notice.chars().count() <= self.left {
                self.text.push_str(&notice);
                self.left -= notice.chars().count();
            } else {
                // The caller reserves the file's own not-shown note. An
                // empty result asks it to use that note instead.
                self.text.clear();
            }
            self.stopped = true;
            false
        }
    }

    pub fn numbered(&mut self, text: &str, range: Option<(u64, u64)>, page: Option<u64>) -> bool {
        let total = text.lines().count() as u64;
        let (first, last) = range.unwrap_or((1, total));
        if first > total {
            return self.line("", &format!("no lines in the range asked; {total} lines"), "use --lines 1");
        }
        for (i, line) in text.lines().enumerate() {
            let n = i as u64 + 1;
            if n < first || n > last {
                continue;
            }
            let resume = match page {
                Some(p) => format!("use --pages {p} --lines {n}-{total}"),
                None => format!("use --lines {n}-{total}"),
            };
            if !self.line(&format!("{n}\t"), line, &resume) {
                return false;
            }
        }
        true
    }

    pub fn finish(self) -> Content {
        Content { text: self.text, stopped: self.stopped }
    }
}

pub(crate) fn visible(text: &str) -> String {
    let mut shown = String::new();
    for c in text.chars() {
        if c.is_control() && c != '\t' {
            for byte in c.encode_utf8(&mut [0; 4]).bytes() {
                shown.push_str(&format!("\\x{byte:02X}"));
            }
        } else {
            shown.push(c);
        }
    }
    shown
}

pub(crate) fn quoted(text: &str) -> String {
    serde_json::to_string(text).expect("a string always serializes")
}
