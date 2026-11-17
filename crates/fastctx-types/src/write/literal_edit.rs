//! Literal edit planning shared by encoded files and synchronized text handlers.
use std::ops::Range;

/// Matching failures describe the requested edit; capacity failures are retryable
/// and must never be treated as a conflicting document revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Match(String),
    BudgetExceeded,
    LengthOverflow,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Match(reason) => f.write_str(reason),
            Self::BudgetExceeded => f.write_str("edited body exceeds its semantic budget"),
            Self::LengthOverflow => f.write_str("edited body length overflow"),
        }
    }
}
impl std::error::Error for Error {}

/// Ranges are UTF-8 byte offsets in already decoded text. The encoding adapter
/// applies only these changes and preserves untouched source bytes.
#[derive(Clone, Debug)]
pub struct Replacement {
    pub text: Range<usize>,
    pub content: String,
    pub context: Range<usize>,
}

pub struct Options<'a> {
    pub all_option: &'a str,
    pub issue: Option<&'a str>,
    pub max_bytes: usize,
}

/// SEARCH treats CRLF as LF, rejects overlapping ambiguity, and preserves the
/// matched region's line endings. Stateful encoding spans retain their context.
pub fn plan(
    text: &str,
    search: &str,
    replacement: &str,
    replace_all: bool,
    options: Options<'_>,
    stateful_at: impl Fn(usize) -> bool,
) -> Result<Vec<Replacement>, Error> {
    let mut changes = Vec::new();
    visit(text, search, replacement, replace_all, options, stateful_at, |change| changes.push(change))?;
    Ok(changes)
}

fn visit(
    text: &str,
    search: &str,
    replacement: &str,
    replace_all: bool,
    options: Options<'_>,
    stateful_at: impl Fn(usize) -> bool,
    mut emit: impl FnMut(Replacement),
) -> Result<(), Error> {
    let option = options.all_option;
    if search.is_empty() {
        return Err(Error::Match("SEARCH is empty; use write to create or replace a whole file".into()));
    }
    // A SEARCH written in a here-document has LF line endings whatever the
    // file has, so both are matched with each CRLF read as LF; the matches
    // are mapped back to the file's own text, whose line endings stay.
    let plain = Plain::of(text);
    let old = search.replace("\r\n", "\n");
    let Some(first) = plain.text.find(&old) else {
        if let Some(issue) = options.issue {
            return Err(Error::Match(format!(
                "SEARCH was not found; the file has lines that cannot be edited: {issue}"
            )));
        }
        // View and edit decode alike, so text copied from `fastctx view`
        // is found; a miss is a difference in the text itself.
        return Err(Error::Match(
            "SEARCH was not found; view the current file and copy its text exactly, including whitespace".into(),
        ));
    };
    // Count overlapping occurrences too: "aa" occurs twice in "aaa".
    let next = first + old.chars().next().expect("nonempty pattern").len_utf8();
    if !replace_all && plain.text[next..].contains(&old) {
        return Err(Error::Match(format!("SEARCH occurs more than once; include more context, or use {option}")));
    }
    let matches =
        plain.text.match_indices(&old).map(|(start, text)| plain.original(start)..plain.original(start + text.len()));
    let dominant = super::text::dominant_ending(text);
    let normalized_replacement = replacement.replace("\r\n", "\n");
    let mut bytes = text.len();
    for matched in matches {
        let original = &text[matched.clone()];
        if original.replace("\r\n", "\n") == normalized_replacement {
            continue;
        }
        let ending = super::text::replacement_ending(original, dominant);
        let replacement = super::text::newlines(replacement, ending);
        let prefix = original
            .chars()
            .zip(replacement.chars())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum::<usize>();
        let suffix = original[prefix..]
            .chars()
            .rev()
            .zip(replacement[prefix..].chars().rev())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum::<usize>();
        // Stateful context may contain switches needed to express the change.
        let (prefix, suffix) = if stateful_at(matched.start) { (0, 0) } else { (prefix, suffix) };
        let replaced = matched.end - matched.start - prefix - suffix;
        let added = replacement.len() - prefix - suffix;
        bytes = bytes.checked_sub(replaced).and_then(|n| n.checked_add(added)).ok_or(Error::LengthOverflow)?;
        if bytes > options.max_bytes {
            return Err(Error::BudgetExceeded);
        }
        emit(Replacement {
            text: matched.start + prefix..matched.end - suffix,
            content: replacement[prefix..replacement.len() - suffix].into(),
            context: matched,
        });
    }

    Ok(())
}

/// Applies one edit to a UTF-8 document after the caller has decoded its schema.
pub fn apply(text: &str, search: &str, replacement: &str, all: bool, max_bytes: usize) -> Result<String, Error> {
    if text.len() > max_bytes {
        return Err(Error::BudgetExceeded);
    }
    let mut output = String::with_capacity(text.len());
    let mut offset = 0;
    visit(
        text,
        search,
        replacement,
        all,
        Options { all_option: "--all", issue: None, max_bytes },
        |_| false,
        |change| {
            output.push_str(&text[offset..change.text.start]);
            output.push_str(&change.content);
            offset = change.text.end;
        },
    )?;
    output.push_str(&text[offset..]);
    Ok(output)
}
/// A text with each CRLF read as LF, and where the LF of each such pair
/// landed in it, so that an offset in it maps back to the text it came from.
struct Plain {
    text: String,
    joined: Vec<usize>,
}

impl Plain {
    fn of(text: &str) -> Self {
        let mut plain = String::with_capacity(text.len());
        let mut joined = Vec::with_capacity(text.matches("\r\n").count());
        let mut last = 0;
        for (at, _) in text.match_indices("\r\n") {
            plain.push_str(&text[last..at]);
            joined.push(plain.len());
            plain.push('\n');
            last = at + 2;
        }
        plain.push_str(&text[last..]);
        Self { text: plain, joined }
    }

    /// The offset in the original text of a boundary in the plain one. A
    /// boundary just before a joined LF lands before its CR, so a match never
    /// starts or ends between the two.
    fn original(&self, offset: usize) -> usize {
        offset + self.joined.partition_point(|&lf| lf < offset)
    }
}
