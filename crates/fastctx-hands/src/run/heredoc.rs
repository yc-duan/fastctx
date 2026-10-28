//! A source scanner for here-documents, not a replacement bash parser.
//!
//! Bash validates grammar. Here we keep source positions, quote removal,
//! command boundaries and pending document queues: information needed to
//! detect hazards which bash intentionally accepts.

use fastctx_types::ipc::Failure;

mod on_only;
pub(super) use on_only::plan as on_only;

#[derive(Default)]
struct Command {
    words: Vec<Word>,
    documents: Vec<usize>,
    grouped: bool,
    /// A redirection takes its standard input elsewhere.
    input_redirected: bool,
    /// A redirection takes its standard output elsewhere, or sends its
    /// standard error where its standard output goes.
    output_redirected: bool,
}

struct Word {
    text: Vec<u8>,
    line: usize,
    offset: usize,
    quoted: bool,
    dynamic: bool,
    assignment: bool,
    pattern: bool,
}

struct Document {
    delimiter: Word,
    strip_tabs: bool,
    end: usize,
    expansion: Option<usize>,
}

struct Scanner<'a> {
    source: &'a [u8],
    offset: usize,
    line: usize,
    commands: Vec<Command>,
    documents: Vec<Document>,
    flows: Vec<(usize, usize)>,
    /// Commands joined by a plain `|`: the first's standard output is the
    /// second's standard input.
    pipes: Vec<(usize, usize)>,
    /// The descriptor written just before a redirection, as `2` in `2>`.
    descriptor: Option<Vec<u8>>,
    word_owner: usize,
    functions: std::collections::HashSet<Vec<u8>>,
}

#[derive(Clone, Copy, PartialEq)]
enum Case {
    Word,
    In,
    Pattern,
    Body,
}

pub(super) fn refuse(line: usize, reason: impl std::fmt::Display) -> Failure {
    Failure::new(format!("line {line}: {reason}."))
}

pub(super) fn check(source: &str) -> Result<(), Failure> {
    let documents = source.contains('<');
    if !documents && !PATH_COMMANDS.iter().any(|name| source.contains(name)) {
        return Ok(());
    }
    let mut scanner = Scanner {
        source: source.as_bytes(),
        offset: 0,
        line: 1,
        commands: Vec::new(),
        documents: Vec::new(),
        flows: Vec::new(),
        word_owner: 0,
        pipes: Vec::new(),
        descriptor: None,
        functions: std::collections::HashSet::new(),
    };
    let scanned = scanner.shell(None);
    // Only a here-document can be beyond the scanner; without one, a script
    // it cannot follow is bash's to read, and its paths the commands' own.
    if !documents && scanned.is_err() {
        return Ok(());
    }
    scanned?;
    let mut uncertain = std::mem::take(&mut scanner.functions);
    for command in &scanner.commands {
        if let (_, Some(name)) = command_kind(&command.words, &std::collections::HashSet::new())
            && name.text == b"alias"
        {
            for word in command.words.iter().filter(|word| word.offset > name.offset) {
                if let Some(equal) = word.text.iter().position(|&byte| byte == b'=') {
                    uncertain.insert(word.text[..equal].to_vec());
                }
            }
        }
    }
    relative_paths(&scanner, &uncertain)?;
    let mut sources = vec![Vec::new(); scanner.commands.len()];
    for &(producer, consumer) in &scanner.flows {
        sources[consumer].push(producer);
    }
    let kinds: Vec<_> = scanner.commands.iter().map(|command| command_kind(&command.words, &uncertain).0).collect();
    let writers = input_sources(
        &sources,
        kinds.iter().enumerate().filter_map(|(index, kind)| (*kind == Kind::Writer).then_some(index)),
    );
    let mut markers = std::collections::HashMap::new();
    for document in &scanner.documents {
        markers.entry(&document.delimiter.text).or_insert(document);
    }
    for (index, command) in scanner.commands.iter().enumerate() {
        let (_, name) = command_kind(&command.words, &uncertain);
        if let Some(name) = name
            && let Some(document) = markers.get(&name.text)
            && document.end < name.offset
        {
            // Where bash ended it: the first line of the body that is the
            // delimiter, which the writer meant as part of the body.
            let ended = 1 + scanner.source[..document.end.min(scanner.source.len())]
                .iter()
                .filter(|&&byte| byte == b'\n')
                .count();
            return Err(refuse(
                ended,
                format!(
                    "the here-document from line {start} ends here, where its body has a line that is only {marker:?}, so the lines after it run as commands, up to line {line}. Use a different quoted delimiter that occurs nowhere in the body",
                    marker = std::str::from_utf8(&name.text).expect("quote removal preserves UTF-8"),
                    start = document.delimiter.line,
                    line = name.line,
                ),
            ));
        }
        // Only a here-document that a write command receives is refused for
        // expanding: anywhere else, expansion is how bash is meant to be
        // used, and a command the source does not name is not presumed to
        // write.
        if writers[index] {
            for &id in &command.documents {
                let document = &scanner.documents[id];
                if let Some(line) = document.expansion {
                    return Err(refuse(
                        line,
                        format!(
                            "bash would expand a here-document passed to a file-writing command (delimiter on line {}). Quote the delimiter, for example <<'CONTENT', to keep the body as written; use printf ... | write ... if expansion is intended",
                            document.delimiter.line,
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// The machine names a script writes literally after `on`, where `on` is
/// the command, each with its line (`world-run.md`, 去别的机器). A name in a
/// variable, or anything inside a here-document's body, which runs where it
/// is sent, is not among them; nor is anything in a script this scanner
/// cannot follow, whose names are checked as each `on` runs.
pub(super) fn on_machines(source: &str) -> Vec<(String, usize)> {
    let mut scanner = Scanner {
        source: source.as_bytes(),
        offset: 0,
        line: 1,
        commands: Vec::new(),
        documents: Vec::new(),
        flows: Vec::new(),
        word_owner: 0,
        pipes: Vec::new(),
        descriptor: None,
        functions: std::collections::HashSet::new(),
    };
    if scanner.shell(None).is_err() {
        return Vec::new();
    }
    let mut named = Vec::new();
    for command in &scanner.commands {
        let mut words = command.words.iter().skip_while(|word| word.assignment);
        let (Some(on), Some(machine)) = (words.next(), words.next()) else { continue };
        if on.text == b"on"
            && !on.quoted
            && !on.dynamic
            && !machine.dynamic
            && !machine.text.starts_with(b"-")
            && let Ok(name) = String::from_utf8(machine.text.clone())
        {
            named.push((name, machine.line));
        }
    }
    named
}

/// The script with each `on` whose output a pipeline hands straight to
/// another `on`, or whose input comes straight from one, written as the
/// prelude's `__fastctx_on_pipe` (`world-run.md`, 数据跟着命令走): its first
/// word says whether its input comes from an `on`, its second names the
/// machine its output goes to, or is `-`. Only literal `on`s joined by a
/// plain `|` qualify, the first not sending its output elsewhere, the
/// second not taking its input elsewhere and running a command; anything
/// else, and any script this scanner cannot follow or that makes `on`
/// something else, is left as written and piped as any command is.
pub(super) fn mark_on_pipes(source: &str) -> String {
    if !source.contains('|') || !source.contains("on") {
        return source.to_string();
    }
    let mut scanner = Scanner {
        source: source.as_bytes(),
        offset: 0,
        line: 1,
        commands: Vec::new(),
        documents: Vec::new(),
        flows: Vec::new(),
        pipes: Vec::new(),
        descriptor: None,
        word_owner: 0,
        functions: std::collections::HashSet::new(),
    };
    if scanner.shell(None).is_err() || scanner.functions.contains(b"on".as_slice()) {
        return source.to_string();
    }
    let aliased = scanner.commands.iter().any(|command| {
        matches!(command_kind(&command.words, &std::collections::HashSet::new()), (_, Some(name)) if name.text == b"alias")
            && command.words.iter().any(|word| word.text.starts_with(b"on="))
    });
    if aliased {
        return source.to_string();
    }
    let bytes = source.as_bytes();
    // A literal `on`: its offset, the machine it names, and how many words
    // follow the machine.
    let literal = |command: &Command| -> Option<(usize, String, usize)> {
        let mut words = command.words.iter().skip_while(|word| word.assignment);
        let (head, machine) = (words.next()?, words.next()?);
        let written = head.text == b"on"
            && !head.quoted
            && !head.dynamic
            && bytes.get(head.offset..head.offset + 2) == Some(b"on".as_slice())
            && bytes.get(head.offset + 2).is_some_and(|byte| *byte == b' ' || *byte == b'\t');
        let name = std::str::from_utf8(&machine.text).ok()?;
        // A machine's name, or `name.hub`: at run time the two ends go
        // straight only when they are of one hub.
        let named = !machine.dynamic && fastctx_types::access::Written::parse(name).is_ok();
        (written && named).then(|| (head.offset, name.to_string(), words.count()))
    };
    let mut marks: std::collections::BTreeMap<usize, (bool, Option<String>)> = std::collections::BTreeMap::new();
    for &(producer, consumer) in &scanner.pipes {
        let (Some((from, _, _)), Some((to, machine, rest))) =
            (literal(&scanner.commands[producer]), literal(&scanner.commands[consumer]))
        else {
            continue;
        };
        if scanner.commands[producer].output_redirected || scanner.commands[consumer].input_redirected || rest == 0 {
            continue;
        }
        marks.entry(from).or_default().1 = Some(machine);
        marks.entry(to).or_default().0 = true;
    }
    let mut marked = source.to_string();
    for (offset, (input, output)) in marks.iter().rev() {
        let call = format!("__fastctx_on_pipe {} {}", u8::from(*input), output.as_deref().unwrap_or("-"));
        marked.replace_range(*offset..*offset + 2, &call);
    }
    marked
}

impl Scanner<'_> {
    fn at(&self, text: &[u8]) -> bool {
        self.source[self.offset..].starts_with(text)
    }

    fn byte(&self) -> Option<u8> {
        self.source.get(self.offset).copied()
    }

    fn advance(&mut self, count: usize) {
        self.line += self.source[self.offset..self.offset + count].iter().filter(|&&b| b == b'\n').count();
        self.offset += count;
    }

    fn command(&mut self) -> usize {
        let index = self.commands.len();
        self.commands.push(Command::default());
        index
    }

    fn shell(&mut self, end: Option<u8>) -> Result<(), Failure> {
        grow(|| self.shell_on_stack(end))
    }

    fn shell_on_stack(&mut self, end: Option<u8>) -> Result<(), Failure> {
        let mut command = self.command();
        let mut pending = Vec::new();
        let mut cases = Vec::new();
        let mut groups = Vec::new();
        let mut function_name = false;
        let mut coprocess = false;
        loop {
            self.word_owner = command;
            match self.byte() {
                None => break,
                Some(b' ' | b'\t') => self.advance(1),
                Some(b'\\') if self.at(b"\\\n") => self.advance(2),
                Some(b'#') => {
                    while self.byte().is_some_and(|b| b != b'\n') {
                        self.advance(1);
                    }
                }
                Some(b'\n') => {
                    self.advance(1);
                    for id in pending.drain(..) {
                        self.body(id)?;
                    }
                    if !self.commands[command].words.is_empty() || !self.commands[command].documents.is_empty() {
                        command = self.command();
                    }
                }
                Some(b')') if cases.last() == Some(&Case::Pattern) => {
                    self.advance(1);
                    *cases.last_mut().expect("case pattern") = Case::Body;
                    command = self.command();
                }
                Some(byte) if Some(byte) == end => {
                    self.advance(1);
                    if !pending.is_empty() {
                        return Err(refuse(
                            self.line,
                            "a here-document crosses a substitution boundary; put its body and closing delimiter before the closing parenthesis",
                        ));
                    }
                    return Ok(());
                }
                Some(b';') => {
                    if (self.at(b";;") || self.at(b";&"))
                        && let Some(case) = cases.last_mut()
                    {
                        *case = Case::Pattern;
                    }
                    while self.byte().is_some_and(|b| b == b';' || b == b'&') {
                        self.advance(1);
                    }
                    command = self.command();
                }
                Some(b'|') if cases.last() == Some(&Case::Pattern) => self.advance(1),
                Some(b'|') if !self.at(b"||") => {
                    self.advance(1);
                    // `|&` pipes standard error too.
                    let both = self.byte() == Some(b'&');
                    if both {
                        self.advance(1);
                    }
                    let consumer = self.command();
                    self.flows.push((command, consumer));
                    if !both {
                        self.pipes.push((command, consumer));
                    }
                    command = consumer;
                }
                Some(b'|' | b'&') => {
                    self.advance(1);
                    if self.byte().is_some_and(|b| b == b'|' || b == b'&') {
                        self.advance(1);
                    }
                    command = self.command();
                }
                Some(b'(') if self.at(b"((") => {
                    self.advance(2);
                    self.balanced(b"))", true)?;
                }
                Some(b'(') => {
                    if self.empty_parentheses()
                        && self.commands[command].words.len() == 1
                        && !self.commands[command].words[0].assignment
                    {
                        self.functions.insert(self.commands[command].words[0].text.clone());
                        self.commands[command].words.clear();
                    }
                    let start = self.commands.len();
                    self.advance(1);
                    if cases.last() != Some(&Case::Pattern) {
                        self.shell(Some(b')'))?;
                    }
                    self.group_flow(start, command);
                }
                Some(b')') => {
                    self.advance(1);
                    command = self.command();
                }
                Some(b'[') if self.at(b"[[") && self.commands[command].words.is_empty() => {
                    self.advance(2);
                    self.balanced(b"]]", false)?;
                }
                Some(b'<' | b'>') if !self.at(b"<(") && !self.at(b">(") => {
                    let line = self.line;
                    let mut operator = Vec::new();
                    while self.byte().is_some_and(|b| b"<>&|".contains(&b)) {
                        operator.push(self.byte().expect("redirection operator"));
                        self.advance(1);
                        while self.at(b"\\\n") {
                            self.advance(2);
                        }
                    }
                    self.redirected(command, &operator);
                    if operator == b"<<" {
                        let strip_tabs = self.byte() == Some(b'-');
                        if strip_tabs {
                            self.advance(1);
                        }
                        self.spaces();
                        let delimiter = self.word(true)?;
                        if delimiter.offset == self.offset {
                            return Err(refuse(
                                line,
                                "cannot identify the here-document delimiter; use a literal quoted word such as <<'CONTENT'",
                            ));
                        }
                        let id = self.documents.len();
                        self.documents.push(Document { delimiter, strip_tabs, end: usize::MAX, expansion: None });
                        self.commands[command].documents.push(id);
                        pending.push(id);
                    } else {
                        self.spaces();
                        self.word(false)?;
                    }
                }
                Some(_) => {
                    self.descriptor = None;
                    let word = self.word(false)?;
                    if word.offset == self.offset {
                        return Err(refuse(
                            self.line,
                            "cannot locate a here-document reliably in this shell construct; separate it into a command with a literal quoted delimiter",
                        ));
                    }
                    let head = self.commands[command].words.is_empty();
                    if function_name {
                        self.functions.insert(word.text.clone());
                        function_name = false;
                        command = self.command();
                        continue;
                    }
                    if coprocess {
                        coprocess = false;
                        let rest = &self.source[self.offset..];
                        let next = rest.iter().copied().find(|byte| !byte.is_ascii_whitespace());
                        if !matches!(word.text.as_slice(), b"{" | b"if" | b"while" | b"for" | b"case")
                            && matches!(next, Some(b'{' | b'('))
                        {
                            continue;
                        }
                    }
                    if cases.last() == Some(&Case::Word) {
                        *cases.last_mut().expect("case header") = Case::In;
                    }
                    if !word.quoted && !word.dynamic {
                        match word.text.as_slice() {
                            b"coproc" if head => {
                                coprocess = true;
                                continue;
                            }
                            b"function" if head => {
                                function_name = true;
                                continue;
                            }
                            b"{" if head => {
                                groups.push((b"}".as_slice(), command, self.commands.len()));
                                command = self.command();
                                continue;
                            }
                            b"}" | b"fi" | b"done" if head => {
                                if let Some((_, group, start)) = groups.pop() {
                                    self.group_flow(start, group);
                                    command = group;
                                } else {
                                    command = self.command();
                                }
                                continue;
                            }
                            b"case" if head => {
                                groups.push((b"esac".as_slice(), command, self.commands.len()));
                                cases.push(Case::Word);
                            }
                            b"if" | b"while" | b"until" | b"for" | b"select" if head => {
                                let end = if word.text == b"if" { b"fi".as_slice() } else { b"done".as_slice() };
                                groups.push((end, command, self.commands.len()));
                                if !matches!(word.text.as_slice(), b"for" | b"select") {
                                    command = self.command();
                                    continue;
                                }
                            }
                            b"in" if cases.last() == Some(&Case::In) => {
                                *cases.last_mut().expect("case header") = Case::Pattern;
                            }
                            b"esac" if head || cases.last() == Some(&Case::Pattern) => {
                                cases.pop();
                                if let Some((_, group, start)) = groups.pop() {
                                    self.group_flow(start, group);
                                    command = group;
                                } else {
                                    command = self.command();
                                }
                                continue;
                            }
                            b"then" | b"elif" | b"else" | b"do" | b"!" if head => {
                                command = self.command();
                                continue;
                            }
                            _ => {}
                        }
                    }
                    if cases.last() == Some(&Case::Pattern) {
                        continue;
                    }
                    // A numeric word immediately before a redirection is its
                    // descriptor, not the command receiving the document.
                    if (word.text.iter().all(u8::is_ascii_digit) || descriptor(&word.text))
                        && !word.quoted
                        && self.byte().is_some_and(|b| b == b'<' || b == b'>')
                    {
                        self.descriptor = Some(word.text);
                        continue;
                    }
                    self.commands[command].words.push(word);
                }
            }
        }
        if let Some(&id) = pending.first() {
            return Err(refuse(
                self.documents[id].delimiter.line,
                "the here-document has no body or closing delimiter; add the closing delimiter on its own line",
            ));
        }
        if end.is_some() {
            return Err(refuse(self.line, "unclosed command substitution; close it before ending the script"));
        }
        Ok(())
    }

    /// Notes what the redirection `operator`, with the descriptor written
    /// before it if any, takes from `command`: its standard input, or its
    /// standard output, which standard error joining it counts as.
    fn redirected(&mut self, command: usize, operator: &[u8]) {
        let number = self
            .descriptor
            .take()
            .map(|text| std::str::from_utf8(&text).ok().and_then(|text| text.parse::<u32>().ok()));
        // No descriptor is the operator's own; one named by a variable,
        // `{fd}>`, could be any.
        let is = |fd: u32| number.is_none_or(|number| number.is_none_or(|number| number == fd));
        if operator.contains(&b'>') && operator != b"<>" {
            if operator.contains(&b'&') || is(1) {
                self.commands[command].output_redirected = true;
            }
        } else if is(0) {
            self.commands[command].input_redirected = true;
        }
    }

    fn empty_parentheses(&self) -> bool {
        let mut rest = &self.source[self.offset + 1..];
        loop {
            match rest.first() {
                Some(b' ' | b'\t' | b'\n') => rest = &rest[1..],
                Some(b'\\') if rest.starts_with(b"\\\n") => rest = &rest[2..],
                Some(b'#') => {
                    let Some(end) = rest.iter().position(|&byte| byte == b'\n') else {
                        return false;
                    };
                    rest = &rest[end + 1..];
                }
                _ => return rest.starts_with(b")"),
            }
        }
    }

    fn group_flow(&mut self, start: usize, group: usize) {
        for member in start..self.commands.len() {
            if !self.commands[member].grouped {
                self.commands[member].grouped = true;
                self.flows.push((member, group));
                self.flows.push((group, member));
            }
        }
    }

    fn spaces(&mut self) {
        loop {
            if self.at(b"\\\n") {
                self.advance(2);
            } else if self.byte().is_some_and(|b| b == b' ' || b == b'\t') {
                self.advance(1);
            } else {
                break;
            }
        }
    }

    fn word(&mut self, delimiter: bool) -> Result<Word, Failure> {
        let mut word = Word {
            text: Vec::new(),
            line: self.line,
            offset: self.offset,
            quoted: false,
            dynamic: false,
            assignment: false,
            pattern: false,
        };
        while let Some(byte) = self.byte() {
            if !delimiter && (self.at(b"$(") || self.at(b"<(") || self.at(b">(")) {
                word.dynamic = true;
                self.substitution()?;
            } else if !delimiter && self.at(b"${") {
                word.dynamic = true;
                self.advance(2);
                self.balanced(b"}", false)?;
            } else if !delimiter && self.at(b"$[") {
                word.dynamic = true;
                self.advance(2);
                self.balanced(b"]", true)?;
            } else if self.at(b"$'") {
                word.quoted = true;
                self.advance(2);
                while let Some(byte) = self.byte() {
                    self.advance(1);
                    if byte == b'\'' {
                        break;
                    }
                    if byte == b'\\' {
                        if delimiter {
                            return Err(refuse(
                                word.line,
                                "escaped ANSI-C here-document delimiters cannot be checked reliably; use a plain quoted delimiter such as <<'CONTENT'",
                            ));
                        }
                        if self.byte().is_some() {
                            self.advance(1);
                        }
                        word.dynamic = true;
                    } else {
                        word.text.push(byte);
                    }
                }
            } else if byte == b'\'' {
                word.quoted = true;
                self.advance(1);
                while let Some(byte) = self.byte() {
                    self.advance(1);
                    if byte == b'\'' {
                        break;
                    }
                    word.text.push(byte);
                }
            } else if byte == b'"' || self.at(b"$\"") {
                word.quoted = true;
                if byte == b'$' {
                    self.advance(1);
                }
                self.advance(1);
                self.double_quote(&mut word, delimiter)?;
            } else if byte == b'\\' {
                self.advance(1);
                if let Some(next) = self.byte() {
                    self.advance(1);
                    if next != b'\n' {
                        word.quoted = true;
                        word.text.push(next);
                    }
                }
            } else if byte == b'`' && !delimiter {
                word.dynamic = true;
                self.backticks()?;
            } else if !delimiter && b"@?*+!".contains(&byte) && self.source.get(self.offset + 1) == Some(&b'(') {
                word.dynamic = true;
                self.advance(2);
                self.balanced(b")", false)?;
            } else if !delimiter && byte == b'[' && array_assignment(&self.source[word.offset..]) {
                word.dynamic = true;
                self.advance(1);
                self.balanced(b"]", true)?;
            } else if b" \t\n;|&()< >".contains(&byte) {
                break;
            } else {
                if !delimiter && matches!(byte, b'$' | b'*' | b'?' | b'~') {
                    word.dynamic = true;
                }
                word.pattern |= !delimiter && matches!(byte, b'[' | b'{');
                word.text.push(byte);
                self.advance(1);
            }
        }
        word.assignment = assignment_source(&self.source[word.offset..self.offset]);
        Ok(word)
    }

    fn double_quote(&mut self, word: &mut Word, delimiter: bool) -> Result<(), Failure> {
        while let Some(byte) = self.byte() {
            if byte == b'"' {
                self.advance(1);
                return Ok(());
            }
            if !delimiter && self.at(b"$(") {
                word.dynamic = true;
                self.substitution()?;
            } else if !delimiter && self.at(b"${") {
                word.dynamic = true;
                self.advance(2);
                self.balanced(b"}", false)?;
            } else if byte == b'`' && !delimiter {
                word.dynamic = true;
                self.backticks()?;
            } else if byte == b'\\' {
                self.advance(1);
                match self.byte() {
                    Some(b'\n') => self.advance(1),
                    Some(next @ (b'$' | b'`' | b'"' | b'\\')) => {
                        word.text.push(next);
                        self.advance(1);
                    }
                    _ => word.text.push(b'\\'),
                }
            } else {
                word.dynamic |= byte == b'$' && !delimiter;
                word.text.push(byte);
                self.advance(1);
            }
        }
        Err(refuse(word.line, "unclosed double quote; close the quote before ending the script"))
    }

    fn substitution(&mut self) -> Result<(), Failure> {
        if self.at(b"$((") {
            self.advance(3);
            self.balanced(b"))", true)
        } else {
            let input = self.byte() == Some(b'<');
            let output = self.byte() == Some(b'>');
            let owner = self.word_owner;
            let start = self.commands.len();
            self.advance(2);
            self.shell(Some(b')'))?;
            self.word_owner = owner;
            if input {
                for producer in start..self.commands.len() {
                    if !self.commands[producer].grouped {
                        self.commands[producer].grouped = true;
                        self.flows.push((producer, owner));
                    }
                }
            }
            if output {
                for consumer in start..self.commands.len() {
                    if !self.commands[consumer].grouped {
                        self.commands[consumer].grouped = true;
                        self.flows.push((owner, consumer));
                    }
                }
            }
            Ok(())
        }
    }

    fn backticks(&mut self) -> Result<(), Failure> {
        let line = self.line;
        self.advance(1);
        let mut body = Vec::new();
        let mut offsets = Vec::new();
        while let Some(byte) = self.byte() {
            let offset = self.offset;
            self.advance(1);
            if byte == b'`' {
                if body.windows(2).any(|pair| pair == b"<<") {
                    return Err(refuse(
                        line,
                        "a here-document inside backticks has another escape layer; replace the backticks with $(...) and use a quoted delimiter",
                    ));
                }
                let mut nested = Scanner {
                    source: &body,
                    offset: 0,
                    line,
                    commands: Vec::new(),
                    documents: Vec::new(),
                    flows: Vec::new(),
                    word_owner: 0,
                    pipes: Vec::new(),
                    descriptor: None,
                    functions: std::collections::HashSet::new(),
                };
                nested.shell(None)?;
                self.functions.extend(nested.functions);
                let start = self.commands.len();
                for mut command in nested.commands {
                    for word in &mut command.words {
                        word.offset = offsets[word.offset];
                        word.line = self.source[..word.offset].iter().filter(|&&b| b == b'\n').count() + 1;
                    }
                    self.commands.push(command);
                }
                self.flows.extend(nested.flows.into_iter().map(|(a, b)| (a + start, b + start)));
                return Ok(());
            }
            if byte == b'\\' && self.byte().is_some_and(|b| b"$`\\\n".contains(&b)) {
                let next = self.byte().expect("escaped character");
                self.advance(1);
                if next != b'\n' {
                    body.push(next);
                    offsets.push(offset);
                }
            } else {
                body.push(byte);
                offsets.push(offset);
            }
        }
        Err(refuse(line, "unclosed backtick substitution; replace it with $(...)"))
    }

    fn balanced(&mut self, end: &[u8], arithmetic: bool) -> Result<(), Failure> {
        grow(|| self.balanced_on_stack(end, arithmetic))
    }

    fn balanced_on_stack(&mut self, end: &[u8], arithmetic: bool) -> Result<(), Failure> {
        let line = self.line;
        // Bash ends `${` at the first brace of its own, whatever parentheses
        // or braces come before it, as in `${version%%(*}` and `${x:-{a}`;
        // only a `${` inside it has a brace of its own.
        let expansion = end == b"}";
        let mut parentheses = 0usize;
        let mut brackets = 0usize;
        while let Some(byte) = self.byte() {
            if parentheses == 0 && brackets == 0 && self.at(end) {
                self.advance(end.len());
                return Ok(());
            }
            if self.at(b"$(") || self.at(b"<(") || self.at(b">(") {
                self.substitution()?;
            } else if expansion && self.at(b"${") {
                self.advance(2);
                self.balanced(b"}", false)?;
            } else if byte == b'`' {
                self.backticks()?;
            } else if byte == b'\'' || byte == b'"' {
                let mut word = Word {
                    text: Vec::new(),
                    line: self.line,
                    offset: self.offset,
                    quoted: true,
                    dynamic: false,
                    assignment: false,
                    pattern: false,
                };
                self.advance(1);
                if byte == b'"' {
                    self.double_quote(&mut word, false)?;
                } else {
                    while self.byte().is_some_and(|b| b != b'\'') {
                        self.advance(1);
                    }
                    if self.byte().is_some() {
                        self.advance(1);
                    }
                }
            } else if byte == b'\\' {
                self.advance(1);
                if self.byte().is_some() {
                    self.advance(1);
                }
            } else {
                match byte {
                    b'(' if !expansion => parentheses += 1,
                    b')' if !expansion => parentheses = parentheses.saturating_sub(1),
                    b'[' if arithmetic || end == b"]" => brackets += 1,
                    b']' if arithmetic || end == b"]" => brackets = brackets.saturating_sub(1),
                    _ => {}
                }
                self.advance(1);
            }
        }
        Err(refuse(
            line,
            "cannot locate the end of an expansion, arithmetic expression or pattern; simplify that construct before using a here-document",
        ))
    }

    fn body(&mut self, id: usize) -> Result<(), Failure> {
        let quoted = self.documents[id].delimiter.quoted;
        let strip_tabs = self.documents[id].strip_tabs;
        let mut expansion = None;
        let body_start = self.offset;
        let body_line = self.line;
        while self.offset < self.source.len() {
            let start = self.offset;
            let prior_expansion = expansion;
            let mut logical = Vec::new();
            loop {
                let line = self.line;
                let mut physical = Vec::new();
                while let Some(byte) = self.byte() {
                    self.advance(1);
                    if byte == b'\n' {
                        break;
                    }
                    physical.push(byte);
                }
                if strip_tabs && logical.is_empty() {
                    let tabs = physical.iter().take_while(|&&byte| byte == b'\t').count();
                    physical.drain(..tabs);
                }
                if !quoted && expansion.is_none() && expands(&physical) {
                    expansion = Some(line);
                }
                // Unquoted backslash-newline joins lines before bash compares
                // a delimiter. An even run of backslashes quotes itself.
                let backslashes = physical.iter().rev().take_while(|&&b| b == b'\\').count();
                let continued =
                    !quoted && backslashes % 2 == 1 && self.source.get(self.offset.wrapping_sub(1)) == Some(&b'\n');
                if continued {
                    physical.pop();
                    expansion.get_or_insert(line);
                }
                logical.extend(physical);
                if !continued || self.offset == self.source.len() {
                    break;
                }
            }
            if logical == self.documents[id].delimiter.text {
                self.documents[id].end = start;
                self.documents[id].expansion = prior_expansion;
                if !quoted {
                    self.body_expansions(body_start, start, body_line)?;
                }
                return Ok(());
            }
        }
        Err(refuse(
            self.documents[id].delimiter.line,
            "the here-document has no closing delimiter; put its delimiter alone on a line, with no spaces",
        ))
    }
    fn body_expansions(&mut self, begin: usize, end: usize, line: usize) -> Result<(), Failure> {
        let (source, offset, after_line) = (self.source, self.offset, self.line);
        self.source = &source[..end];
        self.offset = begin;
        self.line = line;
        while let Some(byte) = self.byte() {
            if self.at(b"$(") {
                self.substitution()?;
            } else if self.at(b"${") {
                self.advance(2);
                self.balanced(b"}", false)?;
            } else if byte == b'`' {
                self.backticks()?;
            } else if byte == b'\\' && self.source.get(self.offset + 1).is_some_and(|b| b"\\$`\n".contains(b)) {
                self.advance(2);
            } else {
                self.advance(1);
            }
        }
        self.source = source;
        self.offset = offset;
        self.line = after_line;
        Ok(())
    }
}

fn grow<T>(scan: impl FnOnce() -> T) -> T {
    // Use stacker's documented allocation policy (docs.rs/stacker): reserve
    // 32 KiB before a recursive descent and grow in 1 MiB segments. These are
    // allocation sizes, not a limit on source size or nesting. In particular,
    // a valid nested substitution must not exhaust a Windows node's stack.
    stacker::maybe_grow(32 * 1024, 1024 * 1024, scan)
}

fn input_sources(sources: &[Vec<usize>], seeds: impl Iterator<Item = usize>) -> Vec<bool> {
    let mut marked = vec![false; sources.len()];
    let mut queue = Vec::new();
    for index in seeds {
        marked[index] = true;
        queue.push(index);
    }
    while let Some(consumer) = queue.pop() {
        for &producer in &sources[consumer] {
            if !marked[producer] {
                marked[producer] = true;
                queue.push(producer);
            }
        }
    }
    marked
}

fn expands(body: &[u8]) -> bool {
    body.iter().enumerate().any(|(index, &byte)| {
        let next = body.get(index + 1).copied();
        byte == b'`'
            || byte == b'$' && next.is_some_and(|b| b.is_ascii_alphanumeric() || b"_@*#?$!-({[".contains(&b))
            || byte == b'\\' && next.is_some_and(|b| b"\\$`".contains(&b))
    })
}

#[derive(PartialEq)]
enum Kind {
    Writer,
    Other,
    Unknown,
}

/// Where the command of `words` is named, past what only runs it: its
/// assignments, `on MACHINE`, `command`, `exec`, `env`. `Err` with the word
/// where that cannot be told, or with none when nothing is named.
fn command_at(words: &[Word], uncertain: &std::collections::HashSet<Vec<u8>>) -> Result<usize, Option<usize>> {
    let mut index = 0;
    while let Some(word) = words.get(index) {
        if word.assignment {
            index += 1;
        } else {
            break;
        }
    }
    loop {
        let Some(word) = words.get(index) else {
            return Err(None);
        };
        if word.dynamic || uncertain.contains(&word.text) {
            return Err(Some(index));
        }
        match word.text.as_slice() {
            b"on" => index += 2,
            b"command" if words.get(index + 1).is_some_and(|word| matches!(word.text.as_slice(), b"-v" | b"-V")) => {
                return Ok(index);
            }
            b"exec" => {
                let at = index;
                index += 1;
                while let Some(option) = words.get(index).filter(|word| word.text.starts_with(b"-")) {
                    match option.text.as_slice() {
                        b"--" => {
                            index += 1;
                            break;
                        }
                        b"-a" => index += 2,
                        b"-c" | b"-l" | b"-cl" | b"-lc" => index += 1,
                        _ => return Err(Some(at)),
                    }
                }
            }
            b"command" | b"builtin" | b"time" => {
                index += 1;
                while words.get(index).is_some_and(|word| word.text.starts_with(b"-")) {
                    index += 1;
                }
            }
            b"env" => {
                index += 1;
                while let Some(word) = words.get(index) {
                    if assignment(&word.text) || word.text == b"--" {
                        index += 1;
                    } else if word.text.starts_with(b"-") {
                        return Err(Some(index));
                    } else {
                        break;
                    }
                }
            }
            _ => break,
        }
    }
    if words.get(index).is_some() { Ok(index) } else { Err(None) }
}

/// The program a command names, without its directory.
fn program(word: &Word) -> &[u8] {
    word.text.rsplit(|&b| b == b'/' || b == b'\\').next().unwrap_or_default()
}

fn command_kind<'a>(words: &'a [Word], uncertain: &std::collections::HashSet<Vec<u8>>) -> (Kind, Option<&'a Word>) {
    let index = match command_at(words, uncertain) {
        Ok(index) => index,
        Err(at) => return (Kind::Unknown, at.map(|at| &words[at])),
    };
    let word = &words[index];
    let name = program(word);
    if matches!(name, b"fastctx" | b"fastctx.exe") && words.get(index + 1).is_some_and(|word| word.dynamic) {
        return (Kind::Unknown, Some(word));
    }
    let writer = matches!(name, b"edit" | b"write" | b"apply_patch")
        || matches!(name, b"fastctx" | b"fastctx.exe")
            && words.get(index + 1).is_some_and(|word| {
                !word.dynamic && matches!(word.text.as_slice(), b"edit" | b"write" | b"apply-patch")
            });
    (if writer { Kind::Writer } else { Kind::Other }, Some(word))
}

/// What the World's commands that take only absolute paths are called in a
/// script: one that names none of them has nothing to check.
const PATH_COMMANDS: [&str; 4] = ["edit", "write", "copy", "view"];

/// Refuses a script that writes a relative path literally where a World
/// command takes only absolute ones (`world-write.md`, `world-view.md`,
/// `world-run.md`): `edit` and `write` their file, `fastctx copy` each end,
/// `fastctx view` each file, here or after `on MACHINE`. A path the script
/// builds as it runs is the command's to refuse.
fn relative_paths(scanner: &Scanner, uncertain: &std::collections::HashSet<Vec<u8>>) -> Result<(), Failure> {
    for command in &scanner.commands {
        let words = &command.words;
        let Ok(index) = command_at(words, uncertain) else { continue };
        let (name, first) = match program(&words[index]) {
            b"edit" => ("edit", index + 1),
            b"write" => ("write", index + 1),
            b"fastctx" | b"fastctx.exe" => match words.get(index + 1) {
                Some(word) if !word.dynamic => match word.text.as_slice() {
                    b"edit" => ("edit", index + 2),
                    b"write" => ("write", index + 2),
                    b"copy" => ("fastctx copy", index + 2),
                    b"view" => ("fastctx view", index + 2),
                    _ => continue,
                },
                _ => continue,
            },
            _ => continue,
        };
        let valued: &[&[u8]] = match name {
            "edit" | "write" => &[b"--encoding"],
            "fastctx view" => &[
                b"--lines",
                b"--chars",
                b"--pages",
                b"--sheet",
                b"--sheet-index",
                b"--range",
                b"--encoding",
                b"--files0-from",
                b"--attachment",
            ],
            _ => &[],
        };
        let mut at = first;
        let mut options = true;
        while let Some(word) = words.get(at) {
            at += 1;
            // What the script builds could be an option with a value: the
            // rest is the command's to read.
            if word.dynamic {
                break;
            }
            if options && word.text == b"--" {
                options = false;
                continue;
            }
            if options && word.text.len() > 1 && word.text.starts_with(b"-") {
                if valued.contains(&word.text.as_slice()) {
                    at += 1;
                }
                continue;
            }
            if let Some(why) = relative(name == "fastctx copy", &String::from_utf8_lossy(&word.text)) {
                return Err(refuse(
                    word.line,
                    format!(
                        "{name} takes only absolute paths, and {why}: name it from the root, as /\u{2026}, or C:/\u{2026} on Windows; nothing of the script ran"
                    ),
                ));
            }
            if matches!(name, "edit" | "write") {
                break;
            }
        }
    }
    Ok(())
}

/// Why `text`, a path given to a World command, is relative; `None` when it
/// is not. Bash makes a leading `~` the home directory; one of a copy's ends
/// may be `MACHINE:PATH`, whose path nothing expands.
fn relative(copy: bool, text: &str) -> Option<String> {
    let rooted = |path: &str| {
        let bytes = path.as_bytes();
        bytes.first().is_some_and(|byte| matches!(byte, b'/' | b'\\'))
            || bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'/' | b'\\')
    };
    if copy
        && let Some((machine, path)) = text.split_once(':')
        && !machine.is_empty()
        && !(machine.len() == 1 && machine.as_bytes()[0].is_ascii_alphabetic())
    {
        return (!rooted(path)).then(|| format!("{text} names {path} on {machine}, which is relative"));
    }
    (!rooted(text) && !text.starts_with('~')).then(|| format!("{text} is relative"))
}

fn assignment(word: &[u8]) -> bool {
    let Some(equal) = word.iter().position(|&byte| byte == b'=') else {
        return false;
    };
    let name = &word[..equal];
    !name.is_empty()
        && (name[0].is_ascii_alphabetic() || name[0] == b'_')
        && name.iter().all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}

fn assignment_source(source: &[u8]) -> bool {
    let name = source.iter().take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_').count();
    if name == 0 || source[0].is_ascii_digit() {
        return false;
    }
    let mut rest = &source[name..];
    if rest.starts_with(b"[") {
        let Some(end) = rest.iter().rposition(|&b| b == b']') else {
            return false;
        };
        rest = &rest[end + 1..];
    }
    rest.starts_with(b"=") || rest.starts_with(b"+=")
}

fn array_assignment(source: &[u8]) -> bool {
    let name = source.iter().take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_').count();
    if name == 0 || source[0].is_ascii_digit() || source.get(name) != Some(&b'[') {
        return false;
    }
    let mut depth = 0;
    for (offset, &byte) in source[name..].iter().enumerate() {
        if byte == b'[' {
            depth += 1;
        }
        if byte == b']' {
            depth -= 1;
            if depth == 0 {
                let rest = &source[name + offset + 1..];
                return rest.starts_with(b"=") || rest.starts_with(b"+=");
            }
        }
    }
    false
}

fn descriptor(word: &[u8]) -> bool {
    word.starts_with(b"{")
        && word.ends_with(b"}")
        && word.len() > 2
        && word[1..word.len() - 1].iter().all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}
