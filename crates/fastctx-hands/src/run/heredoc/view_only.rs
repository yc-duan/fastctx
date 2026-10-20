//! Literal arguments for the attachment reader; none are evaluated by a shell.

use super::Scanner;

pub(in crate::run) fn arguments(source: &str) -> Option<Vec<String>> {
    if source.contains('\0') {
        return None;
    }
    let mut scan = Scanner {
        source: source.as_bytes(),
        offset: 0,
        line: 1,
        commands: Vec::new(),
        documents: Vec::new(),
        flows: Vec::new(),
        pipes: Vec::new(),
        descriptor: None,
        word_owner: 0,
        functions: Default::default(),
    };
    let mut words = Vec::new();
    loop {
        scan.spaces();
        match scan.byte() {
            None => break,
            Some(b'\n') if source[scan.offset..].trim().is_empty() => break,
            Some(b'\n' | b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')' | b'#') => return None,
            _ => {}
        }
        let word = scan.word(false).ok()?;
        if word.offset == scan.offset || word.dynamic || word.pattern || word.assignment {
            return None;
        }
        words.push(String::from_utf8(word.text).ok()?);
    }
    if words.first().map(String::as_str) != Some("fastctx") || words.get(1).map(String::as_str) != Some("view") {
        return None;
    }
    Some(words.into_iter().skip(2).collect())
}
