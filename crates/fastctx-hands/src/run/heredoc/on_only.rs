//! Recognize a sequence of literal `on` commands with the shared source scanner.

use super::{Document, Scanner, refuse};
use fastctx_types::ipc::Failure;

pub(in crate::run) fn plan(source: &str) -> Result<(String, Option<String>), Failure> {
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
    let mut commands = Vec::new();
    loop {
        scan.spaces();
        if scan.byte() == Some(b'#') {
            while scan.byte().is_some_and(|byte| byte != b'\n') {
                scan.advance(1);
            }
        }
        if scan.byte() == Some(b'\n') {
            scan.advance(1);
            continue;
        }
        if scan.byte().is_none() {
            break;
        }
        let line = scan.line;
        let on = scan.word(false)?;
        if on.text != b"on" || on.dynamic || on.pattern {
            return Err(refuse(
                line,
                "expected `on MACHINE`; put local commands, conditions and variables inside a quoted `on` here-document",
            ));
        }
        scan.spaces();
        let machine = scan.word(false)?;
        if machine.text.is_empty() || machine.dynamic || machine.pattern {
            return Err(refuse(machine.line, "the machine after `on` must be a literal name"));
        }
        let machine = String::from_utf8(machine.text).expect("quote removal preserves UTF-8");
        let mut words = Vec::new();
        let mut document = None;
        loop {
            scan.spaces();
            match scan.byte() {
                None | Some(b'\n' | b';' | b'&' | b'|') => break,
                Some(b'#') => {
                    while scan.byte().is_some_and(|byte| byte != b'\n') {
                        scan.advance(1);
                    }
                    break;
                }
                Some(b'<') if scan.at(b"<<") && !scan.at(b"<<<") && document.is_none() => {
                    scan.advance(2);
                    let strip_tabs = scan.byte() == Some(b'-');
                    if strip_tabs {
                        scan.advance(1);
                    }
                    scan.spaces();
                    let delimiter = scan.word(true)?;
                    if !delimiter.quoted || delimiter.text.is_empty() {
                        return Err(refuse(
                            delimiter.line,
                            "quote the here-document delimiter so its body is expanded only on the named machine",
                        ));
                    }
                    let id = scan.documents.len();
                    scan.documents.push(Document { delimiter, strip_tabs, end: usize::MAX, expansion: None });
                    document = Some(id);
                }
                Some(b'<' | b'>' | b'(' | b')') => {
                    return Err(refuse(
                        scan.line,
                        "put redirections and substitutions inside the quoted `on` here-document",
                    ));
                }
                Some(_) => {
                    let word = scan.word(false)?;
                    if word.offset == scan.offset || word.dynamic || word.pattern {
                        return Err(refuse(
                            word.line,
                            format!(
                                "word {:?} expands outside `on`; quote a literal argument or put the expansion inside its here-document",
                                &source[word.offset..scan.offset]
                            ),
                        ));
                    }
                    words.push(String::from_utf8(word.text).expect("quote removal preserves UTF-8"));
                }
            }
        }
        let script = if let Some(id) = document {
            if scan.byte() != Some(b'\n') {
                return Err(refuse(line, "put the `on` here-document header on its own line"));
            }
            scan.advance(1);
            let begin = scan.offset;
            scan.body(id)?;
            let body = &source[begin..scan.documents[id].end];
            let body: String = if scan.documents[id].strip_tabs {
                body.split_inclusive('\n').map(|line| line.trim_start_matches('\t')).collect()
            } else {
                body.to_string()
            };
            if words.is_empty() {
                body
            } else {
                let delimiter = std::str::from_utf8(&scan.documents[id].delimiter.text).expect("a UTF-8 delimiter");
                let command = words.iter().map(|word| quote(word)).collect::<Vec<_>>().join(" ");
                format!("{command} <<{}\n{body}{delimiter}\n", quote(delimiter))
            }
        } else {
            if words.is_empty() {
                return Err(refuse(line, "`on` needs a command or a quoted here-document"));
            }
            words.iter().map(|word| quote(word)).collect::<Vec<_>>().join(" ")
        };
        commands.push((machine, script));
        if document.is_some() {
            continue;
        }
        match scan.byte() {
            None => break,
            Some(b'\n') => scan.advance(1),
            Some(b';') if !scan.at(b";;") && !scan.at(b";&") => scan.advance(1),
            Some(b'&') if scan.at(b"&&") => scan.advance(2),
            Some(b'|') => {
                let count = if scan.at(b"||") { 2 } else { 1 };
                scan.advance(count);
            }
            _ => return Err(refuse(scan.line, "put this shell operator inside a quoted `on` here-document")),
        }
    }
    let Some((machine, script)) = commands.first() else {
        return Err(refuse(1, "expected an `on MACHINE` command"));
    };
    Ok((machine.clone(), (commands.len() == 1).then(|| script.clone())))
}

fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}
