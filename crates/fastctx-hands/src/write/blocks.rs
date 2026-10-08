//! The shell edit command's literal SEARCH/REPLACE format.

const SEARCH: &str = "<<<<<<< SEARCH";
const SEPARATOR: &str = "=======";
const REPLACE: &str = ">>>>>>> REPLACE";

pub(super) struct Block<'a> {
    pub old: &'a str,
    pub new: &'a str,
}

pub(super) fn contains_marker(text: &str) -> bool {
    text.split(['\r', '\n']).any(|line| [SEARCH, SEPARATOR, REPLACE].contains(&line))
}

pub(super) fn parse(input: &str) -> Result<Vec<Block<'_>>, String> {
    let mut lines = input.split_inclusive('\n').enumerate();
    let mut offset = 0;
    let mut blocks = Vec::new();
    while let Some((line_number, line)) = lines.next() {
        offset += line.len();
        let marker = without_newline(line);
        if marker.is_empty() {
            continue;
        }
        let number = blocks.len() + 1;
        if marker != SEARCH {
            return Err(format!(
                "Block {number}, line {}: expected {SEARCH} on a line by itself; send only SEARCH/REPLACE blocks",
                line_number + 1
            ));
        }
        let mut parts = Vec::new();
        for expected in [SEPARATOR, REPLACE] {
            let start = offset;
            let mut end = None;
            for (line_number, line) in lines.by_ref() {
                let line_start = offset;
                offset += line.len();
                let marker = without_newline(line);
                if marker == expected {
                    end = Some(line_start);
                    break;
                }
                if [SEARCH, SEPARATOR, REPLACE].contains(&marker) {
                    return Err(format!(
                        "Block {number}, line {}: found {marker}, expected {expected}. Marker lines cannot be literal content; use write for content containing them",
                        line_number + 1
                    ));
                }
            }
            let end =
                end.ok_or_else(|| format!("Block {number}: missing {expected}; put each marker on a line by itself"))?;
            parts.push(without_newline(&input[start..end]));
        }
        blocks.push(Block { old: parts[0], new: parts[1] });
    }
    if blocks.is_empty() {
        return Err(format!("Block 1: no SEARCH/REPLACE block; start with {SEARCH} on a line by itself"));
    }
    Ok(blocks)
}

fn without_newline(text: &str) -> &str {
    match text.strip_suffix('\n') {
        Some(text) => text.strip_suffix('\r').unwrap_or(text),
        None => text,
    }
}
