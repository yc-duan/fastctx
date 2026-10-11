//! Definition starts recognised after persistent comment and string lexing.

use super::explore::{Entry, Sample, WHOLE};
use regex_lite::Regex;

pub(super) struct Code {
    language: String,
    pattern: Regex,
    block: usize,
    quote: Option<char>,
    triple: bool,
    escape: bool,
    masked: String,
    pending: Option<Entry>,
    signature: String,
    heredoc: Option<String>,
    line_comment: bool,
    raw_hashes: Option<usize>,
    raw_close: Option<usize>,
    verbatim: bool,
    depth: i64,
    start_depth: i64,
    delimiters: i64,
    start_delimiters: i64,
    classes: Vec<(i64, String)>,
    macro_depth: Option<i64>,
    carry: String,
    cpp_open: Option<String>,
    cpp_close: Option<String>,
    cpp_at: usize,
    php: bool,
    php_tag: String,
    ruby_literal: Option<(char, char, usize)>,
    ruby_comment: bool,
    invalid_literal: bool,
    dollar_open: Option<String>,
    dollar_close: Option<String>,
    dollar_at: usize,
    last_code: char,
    hashes: usize,
    hash_base: char,
    quote_count: Option<usize>,
    quote_opening: bool,
    quote_closing: usize,
    raw_escape: Option<usize>,
    javascript: Option<super::javascript::Javascript>,
}

impl Code {
    pub(super) fn new(extension: &str) -> Option<Self> {
        let extension = match extension {
            "cu" | "cuh" | "hxx" | "inl" | "ipp" => "cpp",
            other => other,
        };
        let modifiers = r"(?:(?:pub(?:\([^)]*\))?|public|private|protected|internal|static|final|abstract|sealed|partial|virtual|override|inline|extern|export|default|async|unsafe|const|open|suspend|fun|data|annotation|companion|infix|tailrec|operator|expect|actual|nonisolated|distributed|convenience|mutating|nonmutating|required|readonly|typedef)\s+)*";
        let definitions = match extension {
            "rs" => {
                r"(?:fn\s+[A-Za-z_]\w*|(?:struct|enum|trait|union)\s+[A-Za-z_]\w*|type\s+[A-Za-z_]\w*(?:\s*<[^>]*>)?\s*=|impl(?:\s|<))"
            }
            "py" | "pyi" => r"(?:def\s+[A-Za-z_]\w*\s*\(|class\s+[A-Za-z_]\w*\s*[:(])",
            "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" => {
                r"(?:function\s*\*?\s+[A-Za-z_$][\w$]*\s*\(|(?:class|interface|enum|namespace)\s+[A-Za-z_$][\w$]*|type\s+[A-Za-z_$][\w$]*(?:\s*<[^>]*>)?\s*=|(?:let|var|const)\s+[A-Za-z_$][\w$]*(?:\s*:[^;]+)?\s*=\s*(?:async\s+)?(?:function\b|class\b|\([^)]*\)(?:\s*:[^=]+)?\s*=>|[A-Za-z_$][\w$]*\s*=>))"
            }
            "go" => {
                r"(?:func\s+(?:\([^)]*\)\s*)?[A-Za-z_]\w*\s*(?:\[|\()|type\s+[A-Za-z_]\w*\s+(?:struct|interface)\b)"
            }
            "java" | "cs" => {
                r"(?:(?:class|struct|interface|enum|record|namespace|union)\s+[A-Za-z_]\w*|(?:[A-Za-z_]\w*(?:::\w+)*(?:\s*<[^;=]*>)?[\s*&]+)+[A-Za-z_~]\w*(?:::[A-Za-z_~]\w*)*\s*\()"
            }
            "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" => {
                r"(?:(?:[A-Za-z_]\w*(?:::\w+)*(?:\s*<[^;=]*>)?[\s*&]+)+[A-Za-z_~]\w*(?:::[A-Za-z_~]\w*)*\s*\(|(?P<type>(?:class|struct|enum(?:\s+(?:class|struct))?|namespace|union)(?:\s+[A-Za-z_]\w*|\s*\{)))"
            }
            "kt" | "kts" => {
                r"(?:fun\s+(?:<[^>]+>\s*)?(?:[\w.]+\.)?[A-Za-z_]\w*\s*\(|constructor\s*\(|(?:class|interface|object)\s+[A-Za-z_]\w*)"
            }
            "swift" => {
                r"(?:(?:func|class|struct|enum|protocol|extension|actor)\s+[A-Za-z_]\w*|init[?!]?\s*\(|deinit\b|subscript\s*\()"
            }
            "rb" => r"(?:def\s+(?:self\.)?[A-Za-z_]\w*[!?=]?|(?:class|module)\s+[A-Z]\w*(?:::\w+)*)",
            "php" => r"(?:function\s*&?\s*[A-Za-z_]\w*\s*\(|(?:class|interface|trait|enum)\s+[A-Za-z_]\w*)",
            "sql" => {
                r"(?i:CREATE\s+(?:OR\s+REPLACE\s+)?(?:(?:TEMP(?:ORARY)?|UNIQUE)\s+)?(?:TABLE|VIEW|INDEX|FUNCTION|PROCEDURE|TRIGGER|TYPE|SCHEMA)\s+(?:IF\s+NOT\s+EXISTS\s+)?[A-Za-z_][\w.]*)"
            }
            "sh" | "bash" | "zsh" => r"(?:function\s+[A-Za-z_]\w*(?:\s*\(\s*\))?|[A-Za-z_]\w*\s*\(\s*\))",
            _ => return None,
        };
        Some(Self {
            language: extension.into(),
            pattern: Regex::new(&format!("^{modifiers}{definitions}")).expect("definition pattern"),
            block: 0,
            quote: None,
            triple: false,
            escape: false,
            masked: String::new(),
            pending: None,
            signature: String::new(),
            heredoc: None,
            line_comment: false,
            raw_hashes: None,
            raw_close: None,
            verbatim: false,
            depth: 0,
            start_depth: 0,
            delimiters: 0,
            start_delimiters: 0,
            classes: Vec::new(),
            macro_depth: None,
            carry: String::new(),
            cpp_open: None,
            cpp_close: None,
            cpp_at: 0,
            php: extension != "php",
            php_tag: String::new(),
            ruby_literal: None,
            ruby_comment: false,
            invalid_literal: false,
            dollar_open: None,
            dollar_close: None,
            dollar_at: 0,
            last_code: '\0',
            hashes: 0,
            hash_base: '\0',
            quote_count: None,
            quote_opening: false,
            quote_closing: 0,
            raw_escape: None,
            javascript: matches!(extension, "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx")
                .then(super::javascript::Javascript::new),
        })
    }

    pub(super) fn feed(&mut self, text: &str) {
        let mut combined = std::mem::take(&mut self.carry);
        combined.push_str(text);
        let end = combined.char_indices().rev().nth(2).map_or(0, |(at, _)| at);
        let consumed = self.feed_inner(&combined, end);
        self.carry.push_str(&combined[consumed..]);
    }

    fn feed_inner(&mut self, text: &str, until: usize) -> usize {
        if self.heredoc.is_some() || self.line_comment || self.ruby_comment || self.invalid_literal {
            return text.len();
        }
        let prefix = &text[..until];
        if self.php
            && self.block == 0
            && self.quote.is_none()
            && self.ruby_literal.is_none()
            && self.javascript.as_ref().is_none_or(|js| !js.active())
            && prefix.is_ascii()
            && !prefix.bytes().any(|b| {
                matches!(b, b'"' | b'\'' | b'`' | b'/' | b'#' | b'%' | b'$' | b'?')
                    || self.language == "sql" && b == b'-'
            })
        {
            for byte in prefix.bytes() {
                if let Some(js) = &mut self.javascript {
                    js.code(byte as char);
                }
                match byte {
                    b'{' => {
                        self.depth += 1;
                        self.delimiters += 1;
                    }
                    b'}' => {
                        self.depth -= 1;
                        self.delimiters -= 1;
                    }
                    b'(' | b'[' => self.delimiters += 1,
                    b')' | b']' => self.delimiters -= 1,
                    _ => {}
                }
            }
            self.masked.push_str(&prefix[..prefix.len().min(WHOLE.saturating_sub(self.masked.len()))]);
            if let Some(last) = prefix.bytes().next_back() {
                self.last_code = last as char;
                self.hashes = 0;
            }
            return until;
        }
        let hash = matches!(self.language.as_str(), "py" | "pyi" | "rb" | "sh" | "bash" | "zsh" | "php");
        let mut chars = text.char_indices().peekable();
        while let Some((at, c)) = chars.next() {
            if at >= until {
                return at;
            }
            let next = chars.peek().map(|(_, c)| *c);
            let mut shown = ' ';
            if !self.php {
                self.php_tag.push(c);
                if self.php_tag.len() > 5 {
                    self.php_tag.remove(0);
                }
                if self.php_tag.ends_with("<?php") || self.php_tag.ends_with("<?=") {
                    self.php = true;
                    self.php_tag.clear();
                }
            } else if let Some(js) = &mut self.javascript
                && js.active()
            {
                js.take(c);
            } else if let Some((open, close, depth)) = &mut self.ruby_literal {
                if self.escape {
                    self.escape = false;
                } else if c == '\\' {
                    self.escape = true;
                } else if open != close && c == *open {
                    *depth += 1;
                } else if c == *close {
                    *depth -= 1;
                    if *depth == 0 {
                        self.ruby_literal = None;
                    }
                }
            } else if self.block > 0 {
                if c == '*' && next == Some('/') {
                    chars.next();
                    self.block -= 1;
                } else if matches!(self.language.as_str(), "rs" | "swift" | "kt" | "kts" | "sql")
                    && c == '/'
                    && next == Some('*')
                {
                    chars.next();
                    self.block += 1;
                }
            } else if let Some(quote) = self.quote {
                if let Some(tag) = &mut self.dollar_open {
                    if c == '$' {
                        self.dollar_close = Some(format!("{tag}$"));
                        self.dollar_open = None;
                    } else if tag.len() + c.len_utf8() <= WHOLE {
                        tag.push(c);
                    } else {
                        self.invalid_literal = true;
                        self.dollar_open = None;
                    }
                } else if let Some(tag) = &self.dollar_close {
                    if tag[self.dollar_at..].starts_with(c) {
                        self.dollar_at += c.len_utf8();
                    } else {
                        self.dollar_at = usize::from(c == '$');
                    }
                    if self.dollar_at == tag.len() {
                        self.dollar_close = None;
                        self.dollar_at = 0;
                        self.quote = None;
                    }
                } else if let Some(delimiter) = &mut self.cpp_open {
                    if c == '(' {
                        self.cpp_close = Some(format!("){delimiter}\""));
                        self.cpp_open = None;
                    } else if delimiter.len() == 16 || !c.is_ascii() || c.is_whitespace() || matches!(c, ')' | '\\') {
                        // C++ [lex.string] permits at most sixteen basic delimiter characters.
                        self.invalid_literal = true;
                        self.cpp_open = None;
                    } else {
                        delimiter.push(c);
                    }
                } else if let Some(close) = &self.cpp_close {
                    if close.chars().nth(self.cpp_at) == Some(c) {
                        self.cpp_at += 1;
                    } else {
                        self.cpp_at = usize::from(c == ')');
                    }
                    if self.cpp_at == close.chars().count() {
                        self.cpp_close = None;
                        self.cpp_at = 0;
                        self.quote = None;
                    }
                } else if let Some(hashes) = self.raw_hashes {
                    let mut escaped = false;
                    if self.language == "swift" {
                        if let Some(count) = &mut self.raw_escape {
                            if c == '#' && *count < hashes {
                                *count += 1;
                                escaped = true;
                            } else {
                                escaped = *count == hashes;
                                self.raw_escape = None;
                            }
                        }
                        if !escaped && c == '\\' {
                            self.raw_escape = Some(0);
                            escaped = true;
                        }
                    }
                    let mut following = chars.clone();
                    let closes = !escaped
                        && c == '"'
                        && (!self.triple
                            || following.next().map(|(_, c)| c) == Some('"')
                                && following.next().map(|(_, c)| c) == Some('"'));
                    if escaped {
                        self.raw_close = None;
                    } else if closes {
                        if self.triple {
                            chars.next();
                            chars.next();
                        }
                        if hashes == 0 {
                            self.quote = None;
                            self.raw_hashes = None;
                            self.triple = false;
                        } else {
                            self.raw_close = Some(0);
                        }
                    } else if c == '#' && self.raw_close.is_some() {
                        let count = self.raw_close.unwrap() + 1;
                        if count == hashes {
                            self.quote = None;
                            self.raw_hashes = None;
                            self.raw_close = None;
                            self.triple = false;
                        } else {
                            self.raw_close = Some(count);
                        }
                    } else {
                        self.raw_close = None;
                    }
                } else if let Some(count) = &mut self.quote_count {
                    if self.quote_opening && c == quote {
                        *count += 1;
                    } else {
                        self.quote_opening = false;
                        self.quote_closing = if c == quote { self.quote_closing + 1 } else { 0 };
                        if self.quote_closing == *count {
                            self.quote_count = None;
                            self.quote = None;
                            self.triple = false;
                            self.quote_closing = 0;
                        }
                    }
                } else if self.triple && matches!(self.language.as_str(), "kt" | "kts") {
                    let mut following = chars.clone();
                    if c == quote
                        && following.next().map(|(_, c)| c) == Some(quote)
                        && following.next().map(|(_, c)| c) == Some(quote)
                    {
                        chars.next();
                        chars.next();
                        self.quote = None;
                        self.triple = false;
                    }
                } else if self.verbatim && c == quote && next == Some(quote) {
                    chars.next();
                } else if self.escape {
                    self.escape = false;
                } else if c == '\\' && !self.verbatim {
                    self.escape = true;
                } else if c == quote {
                    if self.triple {
                        if next == Some(quote) {
                            chars.next();
                            if chars.peek().map(|(_, c)| *c) == Some(quote) {
                                chars.next();
                                self.quote = None;
                                self.triple = false;
                            }
                        }
                    } else {
                        self.quote = None;
                    }
                }
            } else if self.language == "php" && c == '?' && next == Some('>') {
                chars.next();
                self.php = false;
            } else if self.language == "rb" && c == '%' && next.is_some_and(|c| "qQwWrxsiI".contains(c)) {
                chars.next();
                if let Some((_, open)) = chars.next() {
                    let close = match open {
                        '(' => ')',
                        '[' => ']',
                        '{' => '}',
                        '<' => '>',
                        _ => open,
                    };
                    self.ruby_literal = Some((open, close, 1));
                }
            } else if self.language == "rb" && c == '/' && self.masked.trim_end().ends_with(['=', '~', '(']) {
                self.ruby_literal = Some(('/', '/', 1));
            } else if self.language == "sql"
                && c == '$'
                && next.is_some_and(|c| c == '$' || c.is_ascii_alphabetic() || c == '_')
            {
                self.quote = Some('$');
                self.dollar_open = Some("$".into());
            } else if c == '#' && hash
                || c == '/' && next == Some('/')
                || self.language == "sql" && c == '-' && next == Some('-')
            {
                self.line_comment = true;
                return text.len();
            } else if c == '/' && next == Some('*') {
                chars.next();
                self.block = 1;
            } else if c == '/' && self.javascript.as_mut().is_some_and(|js| js.start()) {
                // Starting a regex updates expression context; its contents stay masked.
            } else if matches!(c, '"' | '\'' | '`') {
                if let Some(js) = &mut self.javascript {
                    js.literal();
                }
                // Rust lifetime tokens are code, unlike character literals.
                let mut following = chars.clone();
                let character_literal = following.next().is_some() && following.next().map(|(_, c)| c) == Some('\'');
                if c == '\'' && self.language == "rs" && next.is_some_and(|c| c.is_alphabetic()) && !character_literal {
                    shown = c;
                } else {
                    self.quote = Some(c);
                    self.triple = false;
                    if self.language == "sql" && matches!(c, '"' | '`') && next.is_some_and(|next| next != c) {
                        shown = 'Q';
                    }
                    if c == '"'
                        && matches!(self.language.as_str(), "cpp" | "cc" | "cxx" | "h" | "hpp" | "hh")
                        && self.last_code == 'R'
                    {
                        self.cpp_open = Some(String::new());
                    }
                    self.verbatim = self.language == "cs" && self.last_code == '@';
                    if self.language == "rs"
                        && c == '"'
                        && (self.last_code == 'r' || self.hashes > 0 && self.hash_base == 'r')
                    {
                        self.raw_hashes = Some(self.hashes);
                    }
                    if self.language == "swift" && c == '"' && self.hashes > 0 {
                        self.raw_hashes = Some(self.hashes);
                    }
                    if matches!(self.language.as_str(), "py" | "pyi" | "cs" | "java" | "kt" | "kts" | "swift")
                        && !self.verbatim
                        && next == Some(c)
                    {
                        chars.next();
                        if chars.peek().map(|(_, c)| *c) == Some(c) {
                            chars.next();
                            self.triple = true;
                            if self.language == "cs" {
                                self.quote_count = Some(3);
                                self.quote_opening = true;
                            }
                        } else {
                            self.quote = None;
                            self.raw_hashes = None;
                            self.raw_close = None;
                        }
                    }
                }
            } else {
                shown = c;
            }
            if shown == '#' {
                if self.hashes == 0 {
                    self.hash_base = self.last_code;
                }
                self.hashes += 1;
            } else {
                self.hashes = 0;
            }
            self.last_code = shown;
            if let Some(js) = &mut self.javascript {
                js.code(shown);
            }
            if shown == '{' {
                self.depth += 1;
            } else if shown == '}' {
                self.depth -= 1;
            }
            if matches!(shown, '{' | '(' | '[') {
                self.delimiters += 1;
            } else if matches!(shown, '}' | ')' | ']') {
                self.delimiters -= 1;
            }
            if self.masked.len() + shown.len_utf8() <= WHOLE {
                self.masked.push(shown);
            }
        }
        text.len()
    }

    pub(super) fn end_line(&mut self, entry: Entry, sample: &mut Sample) -> bool {
        let carry = std::mem::take(&mut self.carry);
        self.feed_inner(&carry, carry.len());
        self.last_code = '\0';
        self.hashes = 0;
        self.raw_close = None;
        self.raw_escape = None;
        self.cpp_at = 0;
        self.dollar_at = 0;
        self.escape = false;
        self.quote_opening = false;
        self.quote_closing = 0;
        if let Some(js) = &mut self.javascript {
            js.end_line();
        }
        if self.language == "rb" {
            if entry.text.starts_with("=begin") {
                self.ruby_comment = true;
            } else if entry.text.starts_with("=end") {
                self.ruby_comment = false;
            }
        }
        if let Some(delimiter) = &self.heredoc {
            if entry.text.trim() == delimiter
                || self.language == "php" && entry.text.trim().trim_end_matches(';') == delimiter
            {
                self.heredoc = None;
            }
            return false;
        }
        let text = std::mem::take(&mut self.masked);
        let text = text.trim();
        self.line_comment = false;
        let before = self.start_depth;
        self.start_depth = self.depth;
        let before_delimiters = self.start_delimiters;
        self.start_delimiters = self.delimiters;
        if self.depth < before
            || matches!(self.language.as_str(), "go" | "kt" | "kts" | "swift")
                && ["var ", "val ", "let ", "const "].iter().any(|word| text.starts_with(word))
        {
            self.pending = None;
            self.signature.clear();
        }
        self.classes.retain(|(depth, _)| *depth <= before);
        if self.macro_depth.is_some_and(|depth| before_delimiters < depth) {
            self.macro_depth = None;
        }
        static MACRO: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
            Regex::new(r"(?:macro_rules!\s+[A-Za-z_]\w*\s*|[A-Za-z_]\w*(?:::\w+)*\s*!\s*)[\{\(\[]")
                .expect("macro invocation pattern")
        });
        let possible = match self.language.as_str() {
            "rs" => ["fn", "struct", "enum", "trait", "union", "type", "impl"].iter().any(|word| text.contains(word)),
            "py" | "pyi" => text.contains("def") || text.contains("class"),
            "go" => text.contains("func") || text.contains("type"),
            _ => !text.is_empty(),
        };
        let matched = if possible { self.pattern.find(text) } else { None };
        let c_family = matches!(self.language.as_str(), "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh");
        let type_valid = !c_family || type_declaration(&self.pattern, text);
        let reserved = matched.is_some_and(|m| reserved_function(m.as_str()));
        let definition = matched.is_some() && type_valid && !reserved;
        if self.language == "rs"
            && text.contains('!')
            && MACRO.is_match(text)
            && self.macro_depth.is_none()
            && !definition
        {
            self.macro_depth = Some(before_delimiters + 1);
        }
        static CLASS: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
            Regex::new(r"\b(?:class|struct|interface|enum|record|namespace)\s+([A-Za-z_]\w*)").expect("class pattern")
        });
        if text.contains('{')
            && type_valid
            && ["class", "struct", "interface", "enum", "record", "namespace"].iter().any(|word| text.contains(word))
            && let Some(class) = CLASS.captures(text)
        {
            self.classes.push((before + 1, class[1].into()));
        }
        let forbidden = text.split_whitespace().next().is_some_and(|word| {
            matches!(
                word,
                "return" | "throw" | "new" | "await" | "else" | "if" | "for" | "while" | "switch" | "catch" | "using"
            )
        });
        static METHOD: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
            Regex::new(r"^(?:(?:public|private|protected|static|async|override|virtual|final|get|set)\s+)*([A-Za-z_$][\w$]*)\s*\(").expect("method pattern")
        });
        let member = self.classes.last().is_some_and(|(depth, _)| *depth == before);
        let method = member
            && matches!(
                self.language.as_str(),
                "js" | "jsx"
                    | "mjs"
                    | "cjs"
                    | "ts"
                    | "tsx"
                    | "java"
                    | "cs"
                    | "cpp"
                    | "cc"
                    | "cxx"
                    | "c"
                    | "h"
                    | "hpp"
                    | "hh"
            )
            && METHOD.captures(text).is_some_and(|capture| {
                matches!(self.language.as_str(), "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx")
                    || self.classes.last().is_some_and(|(_, name)| *name == capture[1])
            });
        if matches!(self.language.as_str(), "sh" | "bash" | "zsh" | "rb" | "php")
            && let Some((_, delimiter)) = entry.text.split_once("<<")
        {
            let delimiter = delimiter
                .trim_start_matches(['-', '<', ' ', '\t'])
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_matches(['\'', '"']);
            if !delimiter.is_empty() {
                self.heredoc = Some(delimiter.into());
            }
        }
        if !forbidden && self.macro_depth.is_none() && (definition || method) {
            self.pending = Some(entry);
            self.signature.clear();
        }
        if self.pending.is_some() {
            if self.signature.len() + text.len() <= WHOLE {
                self.signature.push_str(text);
                self.signature.push(' ');
            }
            let signature = self.signature.trim();
            if c_family && !type_declaration(&self.pattern, signature) {
                self.pending = None;
                self.signature.clear();
                return false;
            }
            let python = matches!(self.language.as_str(), "py" | "pyi");
            let ruby = self.language == "rb";
            let shell = matches!(self.language.as_str(), "sh" | "bash" | "zsh");
            static ALIAS: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
                Regex::new(r"^(?:(?:export|default|pub(?:\([^)]*\))?)\s+)*type\s+[A-Za-z_]\w*(?:\s*<[^>]*>)?\s*=")
                    .expect("type alias pattern")
            });
            let alias = matches!(self.language.as_str(), "ts" | "tsx" | "rs") && ALIAS.is_match(signature);
            let kotlin_type = matches!(self.language.as_str(), "kt" | "kts")
                && matched.is_some_and(|m| {
                    !m.as_str().ends_with('(')
                        && m.as_str().split_whitespace().any(|word| matches!(word, "class" | "interface" | "object"))
                });
            let body = definition_body(signature, &self.language)
                || python && signature.ends_with(':')
                || ruby
                || self.language == "sql"
                || shell && signature.ends_with('(')
                || alias
                || kotlin_type;
            let declaration = declaration_ended(signature) && !body;
            let rust_type = self.language == "rs"
                && signature.contains(';')
                && ["struct ", "enum ", "union "].iter().any(|word| signature.contains(word));
            if body || rust_type {
                sample.push(self.pending.take().unwrap());
                self.signature.clear();
            } else if declaration {
                self.pending = None;
                self.signature.clear();
            }
        }
        false
    }
}

fn type_declaration(pattern: &Regex, text: &str) -> bool {
    if pattern.find(text).is_some_and(|m| reserved_function(m.as_str())) {
        return false;
    }
    pattern.captures(text).and_then(|c| c.name("type")).is_none_or(|kind| {
        if kind.as_str().ends_with('{') {
            return true;
        }
        let mut tail = text[kind.end()..].trim_start();
        if tail.starts_with('<') {
            let mut depth = 0;
            let close = tail.char_indices().find_map(|(at, c)| {
                if c == '<' {
                    depth += 1;
                } else if c == '>' {
                    depth -= 1;
                    if depth == 0 {
                        return Some(at + 1);
                    }
                }
                None
            });
            let Some(close) = close else {
                return true;
            };
            tail = tail[close..].trim_start();
        }
        if let Some(rest) = tail.strip_prefix("final")
            && rest.chars().next().is_none_or(|c| c.is_whitespace() || c == '{' || c == ':')
        {
            tail = rest.trim_start();
        }
        tail.is_empty() || tail.starts_with(['{', ':'])
    })
}

fn reserved_function(prefix: &str) -> bool {
    let Some(prefix) = prefix.strip_suffix('(') else {
        return false;
    };
    let name = prefix.split_whitespace().next_back().unwrap_or_default();
    ["__attribute__", "__declspec", "decltype", "sizeof", "alignof"].contains(&name)
}

fn declaration_ended(signature: &str) -> bool {
    let (mut parentheses, mut brackets, mut angles) = (0usize, 0usize, 0usize);
    for c in signature.chars() {
        match c {
            '(' => parentheses += 1,
            ')' => parentheses = parentheses.saturating_sub(1),
            '[' => brackets += 1,
            ']' => brackets = brackets.saturating_sub(1),
            '<' if parentheses == 0 && brackets == 0 => angles += 1,
            '>' if parentheses == 0 && brackets == 0 => angles = angles.saturating_sub(1),
            ';' if parentheses == 0 && brackets == 0 && angles == 0 => return true,
            _ => {}
        }
    }
    false
}

fn definition_body(signature: &str, language: &str) -> bool {
    let (mut parentheses, mut brackets, mut angles) = (0usize, 0usize, 0usize);
    let mut object_type = 0usize;
    let mut previous = ' ';
    let mut assignment = false;
    let mut characters = signature.chars().peekable();
    while let Some(c) = characters.next() {
        match c {
            '(' => parentheses += 1,
            ')' => parentheses = parentheses.saturating_sub(1),
            '[' => brackets += 1,
            ']' => brackets = brackets.saturating_sub(1),
            '<' if parentheses == 0 && brackets == 0 => angles += 1,
            '>' if parentheses == 0 && brackets == 0 => angles = angles.saturating_sub(1),
            '=' if parentheses == 0 && brackets == 0 && angles == 0 => {
                if characters.peek() == Some(&'>') {
                    if assignment || language == "cs" {
                        return true;
                    }
                } else {
                    assignment = true;
                }
            }
            ':' if matches!(language, "py" | "pyi") && parentheses == 0 && brackets == 0 && angles == 0 => return true,
            '{' if parentheses == 0 && brackets == 0 && angles == 0 => {
                if object_type > 0 || matches!(language, "ts" | "tsx") && matches!(previous, ':' | '|' | '&') {
                    object_type += 1;
                } else {
                    return !assignment
                        || !matches!(language, "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "java" | "cs");
                }
            }
            '}' if object_type > 0 => object_type -= 1,
            _ => {}
        }
        if !c.is_whitespace() {
            previous = c;
        }
    }
    false
}
