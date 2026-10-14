//! Line-oriented structure recognition with persistent lexical state.

use std::path::Path;

use regex_lite::Regex;

use super::definitions::Code;
use super::explore::{Entry, Sample, WHOLE};

enum Kind {
    Markdown,
    Json(Json),
    Yaml,
    Toml,
    Csv { quoted: bool, rows: u64, content: bool },
    Code(Box<Code>),
    Other,
}

pub(super) struct Outline {
    kind: Kind,
    sample: Sample,
    fence: Option<(char, usize)>,
    previous: Option<Entry>,
    scalar_indent: Option<usize>,
    toml_table: bool,
    value: super::config_outline::Value,
    value_line: u64,
}

impl Outline {
    pub fn new(path: &Path) -> Self {
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        let extension = match name {
            "Cargo.lock" | "Pipfile" | "poetry.lock" | "uv.lock" => "toml".into(),
            ".bashrc" | ".bash_profile" | ".profile" | ".zshrc" => "sh".into(),
            _ => path.extension().and_then(|s| s.to_str()).unwrap_or_default().to_ascii_lowercase(),
        };
        let kind = match extension.as_str() {
            "md" | "markdown" | "mdx" => Kind::Markdown,
            "json" | "ipynb" => Kind::Json(Json::default()),
            "yaml" | "yml" => Kind::Yaml,
            "toml" => Kind::Toml,
            "csv" | "tsv" => Kind::Csv { quoted: false, rows: 0, content: false },
            _ => Code::new(&extension).map_or(Kind::Other, |code| Kind::Code(Box::new(code))),
        };
        Self {
            kind,
            sample: Sample::new(),
            fence: None,
            previous: None,
            scalar_indent: None,
            toml_table: false,
            value: Default::default(),
            value_line: 0,
        }
    }

    pub fn chunk(&mut self, text: &str, line: u64, offset: u64) {
        if matches!(self.kind, Kind::Yaml) && self.value_line != line {
            self.value_line = line;
            let indent = text.len() - text.trim_start().len();
            if !text.trim().is_empty() && self.scalar_indent.is_some_and(|at| indent <= at) {
                self.scalar_indent = None;
            }
        }
        match &mut self.kind {
            Kind::Json(json) => json.feed(text, line, offset, &mut self.sample),
            Kind::Code(code) => code.feed(text),
            Kind::Yaml if self.scalar_indent.is_none() => self.value.feed(text, false),
            Kind::Toml => self.value.feed(text, true),
            Kind::Csv { quoted, content, .. } => {
                for c in text.chars() {
                    if c == '"' {
                        *quoted = !*quoted;
                    }
                    *content = true;
                }
            }
            _ => {}
        }
    }

    pub fn line(&mut self, original: &str, current: &Entry) {
        if matches!(self.kind, Kind::Other) {
            return;
        }
        let mut entry = current.clone();
        entry.text = super::text::visible(original);
        entry.from = 1;
        if entry.text.chars().count() as u64 != current.length {
            entry.text.clear();
        }
        let trimmed = original.trim();
        let mut paragraph = false;
        let yes = match &mut self.kind {
            Kind::Markdown => {
                let start = original.trim_start_matches([' ', '\t']);
                let indent = original.len() - start.len();
                let tabs = original[..indent].contains('\t');
                let marker = start.chars().next().filter(|c| matches!(c, '`' | '~'));
                let count = marker.map_or(0, |c| start.chars().take_while(|next| *next == c).count());
                if indent <= 3 && !tabs && count >= 3 {
                    match self.fence {
                        None => self.fence = marker.map(|c| (c, count)),
                        Some((c, n))
                            if marker == Some(c)
                                && count >= n
                                && start[count..].trim_matches([' ', '\t']).is_empty() =>
                        {
                            self.fence = None;
                        }
                        _ => {}
                    }
                    false
                } else if self.fence.is_some() || indent > 3 || tabs {
                    false
                } else {
                    let hashes = start.bytes().take_while(|c| *c == b'#').count();
                    let atx = (1..=6).contains(&hashes)
                        && start[hashes..].chars().next().is_none_or(|c| matches!(c, ' ' | '\t'));
                    let trimmed = original.trim_matches([' ', '\t']);
                    let setext =
                        !trimmed.is_empty() && (trimmed.chars().all(|c| c == '=') || trimmed.chars().all(|c| c == '-'));
                    if !atx
                        && !trimmed.is_empty()
                        && (trimmed.chars().all(|c| c == '=') || trimmed.chars().all(|c| c == '-'))
                        && let Some(previous) = self.previous.take()
                        && !previous.text.trim().is_empty()
                    {
                        self.sample.push(previous);
                    }
                    paragraph = !atx
                        && !setext
                        && !trimmed.is_empty()
                        && !start.starts_with(['>', '\t'])
                        && !["- ", "+ ", "* "].iter().any(|marker| start.starts_with(marker));
                    let digits = start.bytes().take_while(u8::is_ascii_digit).count();
                    if digits > 0 && (start[digits..].starts_with(". ") || start[digits..].starts_with(") ")) {
                        paragraph = false;
                    }
                    atx
                }
            }
            Kind::Json(json) => {
                if json.end_line(&entry) {
                    self.sample.complete_line(&entry);
                }
                false
            }
            Kind::Yaml => {
                let indent = original.len() - original.trim_start().len();
                if self.scalar_indent.is_some_and(|at| !trimmed.is_empty() && indent <= at) {
                    self.scalar_indent = None;
                }
                let candidate = self.scalar_indent.is_none() && !self.value.inside && indent == 0 && yaml_key(original);
                if self.scalar_indent.is_none() && yaml_scalar(original) {
                    self.scalar_indent = Some(indent);
                }
                candidate
            }
            Kind::Toml => {
                let inside = self.value.inside;
                let table =
                    !inside && trimmed.starts_with('[') && trimmed.split('#').next().unwrap().trim_end().ends_with(']');
                let key = !inside && !self.toml_table && !trimmed.starts_with('#') && assignment(trimmed, '=');
                self.toml_table |= table;
                table || key
            }
            Kind::Csv { quoted, rows, content } => {
                let header = *rows == 0;
                if !*quoted {
                    *rows += 1;
                    *content = false;
                }
                header
            }
            Kind::Code(code) => code.end_line(entry.clone(), &mut self.sample),
            Kind::Other => false,
        };
        if yes {
            self.sample.push(entry.clone());
        }
        if matches!(self.kind, Kind::Yaml | Kind::Toml) {
            self.value.end_line(matches!(self.kind, Kind::Toml));
        }
        if paragraph {
            if self.previous.is_none() {
                self.previous = Some(entry);
            }
        } else {
            self.previous = None;
        }
    }

    pub fn finish(self, budget: usize, cap: usize) -> (u64, Vec<Entry>) {
        self.sample.finish(budget, cap)
    }

    /// The entries found so far.
    pub fn found(&self) -> u64 {
        self.sample.total
    }

    pub fn csv_rows(&self) -> Option<u64> {
        match &self.kind {
            Kind::Csv { rows, content, .. } => Some(rows + u64::from(*content)),
            _ => None,
        }
    }

    pub fn structured(&self) -> bool {
        !matches!(self.kind, Kind::Other)
    }
}

fn assignment(text: &str, delimiter: char) -> bool {
    let mut quote = None;
    let mut escape = false;
    for (at, c) in text.char_indices() {
        if escape {
            escape = false;
        } else if quote == Some('"') && c == '\\' {
            escape = true;
        } else if quote == Some(c) {
            quote = None;
        } else if quote.is_none() && matches!(c, '"' | '\'') {
            quote = Some(c);
        } else if quote.is_none() && c == '#' {
            return false;
        } else if quote.is_none() && c == delimiter {
            return at > 0 && (delimiter != ':' || text[at + 1..].chars().next().is_none_or(char::is_whitespace));
        }
    }
    false
}

fn yaml_key(text: &str) -> bool {
    !text.starts_with(['#', '-', '?', '%', '!', '&', '*']) && assignment(text, ':')
}

fn yaml_scalar(text: &str) -> bool {
    text.split_once(':').is_some_and(|(_, value)| value.trim_start().starts_with(['|', '>']))
}

#[derive(Default)]
struct Json {
    depth: i64,
    root: bool,
    string: bool,
    escape: bool,
    key: bool,
    expecting: bool,
    pending: Option<Entry>,
    emitted_line: Option<u64>,
}

impl Json {
    fn feed(&mut self, text: &str, line: u64, mut position: u64, sample: &mut Sample) {
        let text = if self.string && !self.key && !self.escape {
            let Some(at) = memchr::memchr2(b'"', b'\\', text.as_bytes()) else {
                return;
            };
            position += crate::text::visible_length(&text[..at]) as u64;
            &text[at..]
        } else {
            text
        };
        for c in text.chars() {
            position += crate::text::visible_length(c.encode_utf8(&mut [0; 4])) as u64;
            if self.string {
                if self.key
                    && let Some(pending) = &mut self.pending
                {
                    if pending.text.len() + c.len_utf8() <= WHOLE {
                        pending.text.push(c);
                    } else {
                        self.key = false;
                        pending.text.clear();
                    }
                }
                if self.escape {
                    self.escape = false;
                } else if c == '\\' {
                    self.escape = true;
                } else if c == '"' {
                    self.string = false;
                }
                continue;
            }
            match c {
                '"' => {
                    self.string = true;
                    self.key = self.root && self.depth == 1 && self.expecting;
                    if self.key {
                        self.pending = Some(Entry {
                            number: line,
                            from: position,
                            length: 0,
                            text: "\"".into(),
                            note: String::new(),
                        });
                    }
                }
                '{' | '[' => {
                    if self.depth == 0 {
                        self.root = c == '{';
                    }
                    self.depth += 1;
                    self.expecting = true;
                }
                '}' | ']' => self.depth -= 1,
                ',' if self.depth == 1 => self.expecting = true,
                ':' if self.depth == 1 => {
                    if let Some(entry) = self.pending.take() {
                        sample.push(entry);
                        self.emitted_line = Some(line);
                    }
                    self.expecting = false;
                }
                _ => {}
            }
        }
    }

    fn end_line(&mut self, line: &Entry) -> bool {
        if let Some(entry) = &mut self.pending
            && entry.number == line.number
        {
            if !line.text.is_empty() {
                *entry = line.clone();
            } else {
                entry.length = line.length;
            }
        }
        self.emitted_line.take() == Some(line.number)
    }
}

pub(super) fn timestamp(text: &str) -> Option<String> {
    static PATTERN: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?:^|[\s\[])(\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?(?:Z|[+-]\d{2}:?\d{2})?|\d{2}:\d{2}:\d{2}(?:[.,]\d+)?)").expect("timestamp pattern")
    });
    PATTERN.captures(text).map(|capture| capture[1].to_owned())
}
