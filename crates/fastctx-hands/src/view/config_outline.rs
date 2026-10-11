//! Streaming string and container boundaries for YAML and TOML keys.

#[derive(Default)]
pub(super) struct Value {
    quote: Option<char>,
    triple: bool,
    escape: bool,
    comment: bool,
    depth: i64,
    previous: char,
    space: bool,
    carry: String,
    pub inside: bool,
}

impl Value {
    pub fn feed(&mut self, text: &str, toml: bool) {
        let mut combined = std::mem::take(&mut self.carry);
        combined.push_str(text);
        let until = combined.char_indices().rev().nth(2).map_or(0, |(at, _)| at);
        let consumed = self.scan(&combined, until, toml);
        self.carry.push_str(&combined[consumed..]);
    }

    fn scan(&mut self, text: &str, until: usize, toml: bool) -> usize {
        let mut chars = text.char_indices().peekable();
        while let Some((at, c)) = chars.next() {
            if at >= until {
                return at;
            }
            if self.comment {
                return text.len();
            }
            if let Some(quote) = self.quote {
                if self.escape {
                    self.escape = false;
                } else if c == '\\' && quote == '"' {
                    self.escape = true;
                } else if c == quote {
                    if self.triple {
                        let mut following = chars.clone();
                        if following.next().map(|(_, c)| c) == Some(quote)
                            && following.next().map(|(_, c)| c) == Some(quote)
                        {
                            chars.next();
                            chars.next();
                            self.quote = None;
                            self.triple = false;
                        }
                    } else if !toml && quote == '\'' && chars.peek().map(|(_, c)| *c) == Some(quote) {
                        chars.next();
                    } else {
                        self.quote = None;
                    }
                }
            } else if c == '#' && (toml || self.space || self.previous == '\0') {
                self.comment = true;
            } else if matches!(c, '\'' | '"') && (toml || matches!(self.previous, '\0' | ':' | '[' | '{' | ',' | '-')) {
                self.quote = Some(c);
                let mut following = chars.clone();
                if toml && following.next().map(|(_, c)| c) == Some(c) && following.next().map(|(_, c)| c) == Some(c) {
                    chars.next();
                    chars.next();
                    self.triple = true;
                }
            } else if matches!(c, '[' | '{')
                && (toml || self.depth > 0 || matches!(self.previous, '\0' | ':' | '[' | '{' | ',' | '-'))
            {
                self.depth += 1;
            } else if matches!(c, ']' | '}') && self.depth > 0 {
                self.depth -= 1;
            }
            self.space = c.is_whitespace();
            if !c.is_whitespace() || c == '#' {
                self.previous = c;
            }
        }
        text.len()
    }

    pub fn end_line(&mut self, toml: bool) {
        let carry = std::mem::take(&mut self.carry);
        self.scan(&carry, carry.len(), toml);
        self.comment = false;
        self.escape = false;
        self.previous = '\0';
        self.space = false;
        self.inside = self.quote.is_some() || self.depth > 0;
    }
}
