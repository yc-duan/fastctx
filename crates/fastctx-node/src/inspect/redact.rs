//! Command lines as a look shows them, with what looks like a secret
//! hidden: a key handed to a program on its command line is no more a
//! model's business than one kept in a file. Each argument is judged by the
//! name it is given under and by its own form.

/// Words in an argument's or a variable's name that mark its value as a
/// secret.
const NAMED: &[&str] =
    &["key", "token", "secret", "password", "passwd", "pass", "credential", "auth", "cookie", "session"];
/// How the keys of well-known services begin.
const PREFIXES: &[&str] = &[
    "sk-",
    "sk_live_",
    "rk_live_",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xapp-",
    "hf_",
    "npm_",
    "AKIA",
    "ASIA",
    "AIza",
    "eyJ",
];
/// The shortest such key; shorter words with those beginnings are words.
const SHORTEST: usize = 20;
/// Authorization schemes, whose next word is the credential itself.
const SCHEMES: &[&str] = &["bearer", "basic", "token", "digest"];
const HIDDEN: &str = "***";

/// How deep scripts quoted within a command line (`sh -c '…'`) are looked
/// into.
const NESTED: u8 = 4;

/// `line` with its secrets hidden, its spacing and quoting kept.
pub(super) fn command(line: &str) -> String {
    within(line, 0)
}

/// `line`, found `depth` quotations deep, with its secrets hidden.
fn within(line: &str, depth: u8) -> String {
    let mut out = String::with_capacity(line.len());
    let mut hide = 0;
    for (gap, word) in words(line) {
        out.push_str(gap);
        if hide > 0 {
            hide -= 1;
            if SCHEMES.contains(&unquoted(word).to_lowercase().as_str()) {
                hide = 1;
            }
            out.push_str(HIDDEN);
            out.push_str(ending(word));
            continue;
        }
        let (shown, next) = argument(word, depth);
        out.push_str(&shown);
        hide = next;
    }
    out
}

/// `line` split into words as a shell or Windows splits a command line,
/// quotes keeping blank space within a word; each with the space before it.
fn words(line: &str) -> Vec<(&str, &str)> {
    let mut found = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        let start = rest.find(|character: char| !character.is_whitespace()).unwrap_or(rest.len());
        let (gap, word) = rest.split_at(start);
        if word.is_empty() {
            break;
        }
        let mut quote = None;
        let mut escaped = false;
        let mut end = word.len();
        for (at, character) in word.char_indices() {
            match (quote, character) {
                (Some('"'), _) if escaped => escaped = false,
                (Some('"'), '\\') => escaped = true,
                (Some(open), _) if character == open => quote = None,
                (Some(_), _) => {}
                (None, '"' | '\'') => quote = Some(character),
                (None, _) if character.is_whitespace() => {
                    end = at;
                    break;
                }
                (None, _) => {}
            }
        }
        found.push((gap, &word[..end]));
        rest = &word[end..];
    }
    found
}

/// One argument as it is shown, and how many words after it are secrets: a
/// secret's name and its value, whole however many words the value holds,
/// are judged first; past that, a quoted argument holding several words, a
/// script handed to a shell, is itself a command line.
fn argument(word: &str, depth: u8) -> (String, usize) {
    let quote = word.chars().next().filter(|first| *first == '"' || *first == '\'');
    let closed = |shown: String| match quote {
        Some(quote) if word.len() > 1 && word.ends_with(quote) => format!("{shown}{quote}"),
        _ => shown,
    };
    if let Some((name, _)) = word.split_once('=')
        && secret_name(name)
        && !name.contains([':', ' ', '\t'])
    {
        return (closed(format!("{name}={HIDDEN}{}", ending(word))), 0);
    }
    let bare = unquoted(word);
    if let Some((name, value)) = bare.split_once(':')
        && !name.is_empty()
        && !name.contains(['/', ' ', '\t'])
        && secret_name(name)
        && !value.starts_with("//")
    {
        let opened = quote.map(String::from).unwrap_or_default();
        // A header written apart from its value hides the words after it.
        if value.trim().is_empty() {
            return (closed(format!("{opened}{name}:")), 1);
        }
        return (closed(format!("{opened}{name}: {HIDDEN}")), 0);
    }
    if let Some(quote) = quote
        && depth < NESTED
        && word.len() > 1
        && word.ends_with(quote)
        && word[1..word.len() - 1].contains(char::is_whitespace)
    {
        return (format!("{quote}{}{quote}", within(&word[1..word.len() - 1], depth + 1)), 0);
    }
    if bare.starts_with('-') && secret_name(bare) {
        return (word.to_string(), 1);
    }
    if SCHEMES.contains(&bare.to_lowercase().as_str()) {
        return (word.to_string(), 1);
    }
    if looks_like_key(bare) {
        return (HIDDEN.to_string(), 0);
    }
    (without_login(word), 0)
}

/// What ends a shell's command at the end of `word` (`;`, `&&`, `|`, `)`),
/// kept when the value before it is hidden.
fn ending(word: &str) -> &str {
    &word[word.trim_end_matches([';', '&', '|', ')']).len()..]
}

/// Whether `name`, an argument's or a variable's, marks its value as a
/// secret.
fn secret_name(name: &str) -> bool {
    let name = unquoted(name).trim_start_matches('-').to_lowercase();
    NAMED.iter().any(|word| name.contains(word))
}

/// Whether `word` has the form of a well-known service's key.
fn looks_like_key(word: &str) -> bool {
    word.len() >= SHORTEST
        && PREFIXES.iter().any(|prefix| word.starts_with(prefix))
        && word.chars().all(|character| character.is_ascii_alphanumeric() || "-_.+/=".contains(character))
}

/// `word` with the login a URL in it carries (`https://user:key@host`)
/// hidden.
fn without_login(word: &str) -> String {
    let Some(scheme) = word.find("://") else { return word.to_string() };
    let start = scheme + 3;
    let authority = word[start..].find(['/', '?', '#', '"', '\'']).map_or(word.len(), |end| start + end);
    match word[start..authority].rfind('@') {
        Some(at) => format!("{}{HIDDEN}{}", &word[..start], &word[start + at..]),
        None => word.to_string(),
    }
}

/// `word` without the quotes around it or within it.
fn unquoted(word: &str) -> &str {
    word.trim_matches(['"', '\''])
}
