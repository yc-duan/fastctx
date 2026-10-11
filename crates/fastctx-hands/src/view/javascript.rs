//! Expression context needed to mask JavaScript regular-expression literals.

pub(super) struct Javascript {
    active: bool,
    class: bool,
    escape: bool,
    operand: bool,
    word: String,
    in_word: bool,
    property: bool,
    control: bool,
    parentheses: Vec<bool>,
    previous: char,
}

impl Javascript {
    pub fn new() -> Self {
        Self {
            active: false,
            class: false,
            escape: false,
            operand: true,
            word: String::new(),
            in_word: false,
            property: false,
            control: false,
            parentheses: Vec::new(),
            previous: ' ',
        }
    }

    pub fn active(&self) -> bool {
        self.active
    }

    pub fn start(&mut self) -> bool {
        self.end_word();
        if self.operand {
            self.literal();
            self.active = true;
        }
        self.active
    }

    pub fn take(&mut self, c: char) {
        if self.escape {
            self.escape = false;
        } else if c == '\\' {
            self.escape = true;
        } else if c == '[' {
            self.class = true;
        } else if c == ']' {
            self.class = false;
        } else if c == '/' && !self.class {
            self.active = false;
        }
    }

    pub fn literal(&mut self) {
        self.end_word();
        self.operand = false;
        self.control = false;
        self.previous = 'v';
    }

    pub fn code(&mut self, c: char) {
        if c.is_alphanumeric() || matches!(c, '_' | '$') {
            if !self.in_word {
                self.word.clear();
                self.property = self.previous == '.';
            }
            // One byte beyond the longest relevant keyword distinguishes an identifier suffix.
            if self.word.len() <= "instanceof".len() {
                self.word.push(c);
            }
            self.in_word = true;
            self.operand = false;
        } else {
            self.end_word();
            if c.is_whitespace() {
                return;
            }
            self.operand = match c {
                '(' => {
                    self.parentheses.push(self.control);
                    true
                }
                ')' => self.parentheses.pop().unwrap_or(false),
                '.' | ']' | '}' => false,
                '+' | '-' if self.previous == c => false,
                '!' => self.operand,
                '=' | '[' | '{' | ':' | ';' | ',' | '?' | '+' | '-' | '*' | '/' | '%' | '&' | '|' | '^' | '~' | '<'
                | '>' => true,
                _ => false,
            };
            self.control = false;
        }
        self.previous = c;
    }

    pub fn end_line(&mut self) {
        self.end_word();
        // ECMAScript regular-expression bodies cannot contain a line terminator.
        self.active = false;
        self.class = false;
        self.escape = false;
    }

    fn end_word(&mut self) {
        if self.in_word {
            self.operand = !self.property
                && matches!(
                    self.word.as_str(),
                    "return"
                        | "throw"
                        | "yield"
                        | "case"
                        | "delete"
                        | "void"
                        | "typeof"
                        | "instanceof"
                        | "in"
                        | "of"
                        | "await"
                        | "else"
                        | "do"
                );
            self.control =
                !self.property && matches!(self.word.as_str(), "if" | "for" | "while" | "switch" | "with" | "catch");
            self.in_word = false;
        }
    }
}
