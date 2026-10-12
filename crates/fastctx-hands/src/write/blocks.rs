//! The shell edit command's literal SEARCH/REPLACE format.
//!
//! The markers are Aider's, `<<<<<<< SEARCH`, `=======` and
//! `>>>>>>> REPLACE`, and may be written longer. The three markers of a block
//! have the length of its SEARCH line, the count of its `<`, and only whole
//! lines of exactly that length are markers to that block; any other line is
//! text. So text that itself holds marker lines, such as a document about this
//! format, is edited with markers longer than those lines, and no line of the
//! input can be read both as text and as a boundary.

use std::collections::BTreeSet;

/// The length of the markers as Aider writes them, and the shortest taken.
const ORDINARY: usize = 7;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Search,
    Separator,
    Replace,
}

/// A whole line that is a marker, of some length.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Marker {
    kind: Kind,
    length: usize,
}

impl Marker {
    /// The marker `line`, without its line ending, is, if it is one.
    fn of(line: &str) -> Option<Self> {
        let (kind, fill, word) = match line.as_bytes().first()? {
            b'<' => (Kind::Search, '<', " SEARCH"),
            b'=' => (Kind::Separator, '=', ""),
            b'>' => (Kind::Replace, '>', " REPLACE"),
            _ => return None,
        };
        let length = line.len() - line.trim_start_matches(fill).len();
        (length >= ORDINARY && &line[length..] == word).then_some(Self { kind, length })
    }

    fn text(self) -> String {
        match self.kind {
            Kind::Search => format!("{} SEARCH", "<".repeat(self.length)),
            Kind::Separator => "=".repeat(self.length),
            Kind::Replace => format!("{} REPLACE", ">".repeat(self.length)),
        }
    }
}

pub(super) struct Block<'a> {
    pub old: &'a str,
    pub new: &'a str,
    /// The length of the block's markers; none for an edit given as fields.
    pub markers: Option<usize>,
}

pub(super) fn parse(input: &str) -> Result<Vec<Block<'_>>, String> {
    let mut lines = input.split_inclusive('\n').enumerate();
    let mut offset = 0;
    let mut blocks = Vec::new();
    while let Some((index, line)) = lines.next() {
        offset += line.len();
        let text = crate::text::line_content(line);
        if text.is_empty() {
            continue;
        }
        let number = blocks.len() + 1;
        let search_line = index + 1;
        let Some(Marker { kind: Kind::Search, length }) = Marker::of(text) else {
            return Err(format!(
                "Block {number}, line {search_line}: expected {} on a line by itself; send only SEARCH/REPLACE blocks",
                Marker { kind: Kind::Search, length: ORDINARY }.text()
            ));
        };
        let terminator = if line.ends_with("\r\n") { "\r\n" } else { "\n" };
        let mut parts = Vec::new();
        for kind in [Kind::Separator, Kind::Replace] {
            let expected = Marker { kind, length };
            let start = offset;
            let mut end = None;
            for (index, line) in lines.by_ref() {
                let line_start = offset;
                offset += line.len();
                let text = line.strip_suffix(terminator).unwrap_or(line);
                match Marker::of(text) {
                    Some(found) if found == expected => {
                        end = Some(line_start);
                        break;
                    }
                    Some(found) if found.length == length => {
                        return Err(format!(
                            "Block {number}, line {}: found {text}, expected {}. If that line is text, write the block's markers longer than every line like them in your input, all three the same length: {}",
                            index + 1,
                            expected.text(),
                            longer(longest(input)),
                        ));
                    }
                    _ => {}
                }
            }
            let end = end.ok_or_else(|| {
                format!(
                    "Block {number}: no {} follows its {} on line {search_line}; the three markers of a block have the same length, each on a line by itself",
                    expected.text(),
                    Marker { kind: Kind::Search, length }.text(),
                )
            })?;
            parts.push(input[start..end].strip_suffix(terminator).unwrap_or(&input[start..end]));
        }
        blocks.push(Block { old: parts[0], new: parts[1], markers: Some(length) });
    }
    if blocks.is_empty() {
        return Err(format!(
            "Block 1: no SEARCH/REPLACE block; start with {} on a line by itself",
            Marker { kind: Kind::Search, length: ORDINARY }.text()
        ));
    }
    Ok(blocks)
}

/// Why text copied from `file` could be read as a boundary of these blocks:
/// the file has whole lines that are markers of a length one of them uses.
/// Lines are numbered as `view` numbers them.
pub(super) fn ambiguity(file: &str, blocks: &[Block<'_>], input: &str) -> Option<String> {
    let used: BTreeSet<usize> = blocks.iter().filter_map(|block| block.markers).collect();
    let mut colliding = Vec::new();
    let mut longest_in_file = 0;
    for (index, line) in file.split_inclusive('\n').enumerate() {
        if let Some(marker) = Marker::of(crate::text::line_content(line)) {
            longest_in_file = longest_in_file.max(marker.length);
            if used.contains(&marker.length) {
                colliding.push(index + 1);
            }
        }
    }
    if colliding.is_empty() {
        return None;
    }
    let lines = match colliding.as_slice() {
        [line] => format!("line {line} is a SEARCH/REPLACE marker"),
        [rest @ .., last] => format!(
            "lines {} and {last} are SEARCH/REPLACE markers",
            rest.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
        ),
        [] => unreachable!("not empty"),
    };
    Some(format!(
        "{lines} of the length your markers have, so text copied from the file could be read as a block boundary. Write the markers longer than every line like them in the file, all three of a block the same length: {}",
        longer(longest_in_file.max(longest(input)))
    ))
}

/// The length of the longest marker line in `text`, or 0.
fn longest(text: &str) -> usize {
    text.split_inclusive('\n')
        .filter_map(|line| Marker::of(crate::text::line_content(line)))
        .map(|m| m.length)
        .max()
        .unwrap_or(0)
}

/// Markers one longer than `length`, as a model would write them.
fn longer(length: usize) -> String {
    let length = length.max(ORDINARY) + 1;
    [Kind::Search, Kind::Separator, Kind::Replace].map(|kind| Marker { kind, length }.text()).join(", ")
}
