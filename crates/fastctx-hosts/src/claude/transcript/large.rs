//! A record line too large to hold whole (sync-v2-impl-design §18, the
//! large single value): read in parts, its bytes passed through a cut that
//! keeps every token of the JSON but the long contents of its strings. A
//! string past `LONG` bytes keeps its first `LONG`, cut where a character
//! and an escape end, and a mark of how much was left out; once what is kept
//! passes `KEPT`, later strings keep only `SHORT`; what is kept never passes
//! `MOST` — a line whose structure alone is larger than that is not read.
//! The same bytes give the same cut, read in whatever parts.

/// Lines at least this long are read in parts.
pub(crate) const LARGE: usize = 8 << 20;
const LONG: usize = 256 << 10;
const KEPT: usize = 4 << 20;
const SHORT: usize = 1 << 10;
const MOST: usize = 16 << 20;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Escape {
    None,
    Backslash,
    /// Inside `\u`, how many hex digits are still to come.
    Unicode(u8),
}

/// The cut of one line, fed its bytes in order.
pub(crate) struct Cut {
    kept: Vec<u8>,
    in_string: bool,
    escape: Escape,
    /// The hex digits of the `\u` escape under way.
    unit: u16,
    /// The last escape was a leading surrogate, its trailing one to come:
    /// no cut between them.
    leading: bool,
    /// Bytes of the current string kept, and its limit.
    length: usize,
    limit: usize,
    left_out: u64,
    /// Kept passed `MOST`: the line is not read.
    over: bool,
}

impl Cut {
    pub(crate) fn new() -> Self {
        Self {
            kept: Vec::new(),
            in_string: false,
            escape: Escape::None,
            unit: 0,
            leading: false,
            length: 0,
            limit: LONG,
            left_out: 0,
            over: false,
        }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.byte(byte);
        }
    }

    fn keep(&mut self, byte: u8) {
        if self.kept.len() >= MOST {
            self.over = true;
            return;
        }
        self.kept.push(byte);
    }

    fn byte(&mut self, byte: u8) {
        if !self.in_string {
            self.keep(byte);
            if byte == b'"' {
                self.in_string = true;
                self.length = 0;
                self.left_out = 0;
                self.limit = if self.kept.len() > KEPT { SHORT } else { LONG };
            }
            return;
        }
        if self.escape == Escape::None && byte == b'"' {
            if self.left_out > 0 {
                let mark = format!("…[{} bytes not shown]", self.left_out);
                for byte in mark.into_bytes() {
                    self.keep(byte);
                }
            }
            self.keep(byte);
            self.in_string = false;
            return;
        }
        // A character begins here when no escape is under way, the byte is
        // not a UTF-8 continuation, and no trailing surrogate is owed.
        let begins = self.escape == Escape::None && byte & 0xC0 != 0x80 && !self.leading;
        if self.left_out > 0 || (begins && self.length >= self.limit) {
            self.left_out += 1;
        } else {
            self.keep(byte);
            self.length += 1;
        }
        self.escape = match self.escape {
            Escape::None if byte == b'\\' => Escape::Backslash,
            Escape::None => Escape::None,
            Escape::Backslash if byte == b'u' => {
                self.unit = 0;
                Escape::Unicode(4)
            }
            Escape::Backslash => {
                self.leading = false;
                Escape::None
            }
            Escape::Unicode(left) => {
                let digit = (byte as char).to_digit(16).unwrap_or(0) as u16;
                self.unit = (self.unit << 4) | digit;
                if left > 1 {
                    Escape::Unicode(left - 1)
                } else {
                    self.leading = (0xD800..=0xDBFF).contains(&self.unit);
                    Escape::None
                }
            }
        };
        // Characters other than an escape end a surrogate pair's wait too:
        // what follows a lone leading surrogate is as the record has it.
        if self.escape == Escape::None && byte != b'\\' && !byte.is_ascii_hexdigit() {
            self.leading = false;
        }
    }

    /// What is kept, or none when the line's structure alone is too large.
    pub(crate) fn finish(self) -> Option<Vec<u8>> {
        (!self.over).then_some(self.kept)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn cut(bytes: &[u8], parts: usize) -> Option<Vec<u8>> {
        let mut cut = Cut::new();
        for part in bytes.chunks(parts.max(1)) {
            cut.feed(part);
        }
        cut.finish()
    }

    #[test]
    fn short_strings_and_tokens_pass_unchanged() {
        let line = serde_json::to_vec(&json!({"type": "user", "uuid": "u1", "n": [1, 2.5, true, null], "t": "a \"q\" \\ \u{e9} \u{1F600}"})).unwrap();
        assert_eq!(cut(&line, 3).unwrap(), line);
    }

    #[test]
    fn a_long_string_is_cut_with_its_mark_and_stays_json() {
        let long = "é".repeat(LONG); // two bytes each: the cut falls between characters
        let line = serde_json::to_vec(&json!({"type": "user", "message": {"content": long}, "uuid": "u2"})).unwrap();
        for parts in [1, 7, 4096, line.len()] {
            let kept = cut(&line, parts).unwrap();
            let value: Value = serde_json::from_slice(&kept).unwrap();
            let content = value["message"]["content"].as_str().unwrap();
            assert!(content.starts_with("éé"));
            assert!(content.ends_with(&format!("…[{} bytes not shown]", 2 * LONG - LONG)));
            assert_eq!(value["uuid"], "u2");
        }
    }

    #[test]
    fn escapes_and_surrogate_pairs_are_never_split() {
        // Escaped quotes, backslashes and a surrogate pair across the limit.
        let mut text = "x".repeat(LONG - 3);
        text.push_str("\u{1F600}\"\\");
        text.push_str(&"y".repeat(100));
        let line = serde_json::to_vec(&json!({"s": text})).unwrap();
        let ascii = String::from_utf8(line.clone()).unwrap().replace('\u{1F600}', "\\ud83d\\ude00").into_bytes();
        for input in [&line, &ascii] {
            for parts in [1, 2, 3, 5, 64] {
                let kept = cut(input, parts).unwrap();
                let value: Value = serde_json::from_slice(&kept).expect("still JSON");
                assert!(value["s"].as_str().unwrap().contains("not shown"));
            }
        }
    }

    #[test]
    fn strings_after_much_is_kept_are_cut_short() {
        let strings: Vec<String> = (0..40).map(|n| format!("{n}{}", "z".repeat(LONG))).collect();
        let line = serde_json::to_vec(&json!({"many": strings})).unwrap();
        let kept = cut(&line, 65536).unwrap();
        assert!(kept.len() < KEPT + 40 * (SHORT + 64) + LONG);
        let value: Value = serde_json::from_slice(&kept).unwrap();
        assert_eq!(value["many"].as_array().unwrap().len(), 40);
    }

    /// A record with a line past `LARGE` in it: read whole, the line's long
    /// text is cut and the seam still moves past it; read on from the seam
    /// after more is written, the prefix it hashed in parts holds, and only
    /// the new rows are read.
    #[test]
    fn a_follower_reads_past_a_large_line_and_on_from_its_seam() {
        use crate::claude::transcript::follow;
        use std::collections::HashSet;
        use std::io::Write;
        fn row(n: usize, text: &str) -> String {
            let at = format!("2026-11-12T01:00:{:02}.000Z", n);
            let value = if n.is_multiple_of(2) {
                json!({"type": "user", "uuid": format!("u{n}"), "parentUuid": null, "sessionId": "large", "timestamp": at,
                       "message": {"role": "user", "content": text}})
            } else {
                json!({"type": "assistant", "uuid": format!("a{n}"), "parentUuid": format!("u{}", n - 1), "sessionId": "large",
                       "timestamp": at, "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}})
            };
            format!("{value}\n")
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("record.jsonl");
        let large = "w".repeat(LARGE + (1 << 20));
        std::fs::write(&path, [row(0, "first"), row(1, "an answer"), row(2, &large), row(3, "another"), row(4, "words")].concat()).unwrap();
        let earlier = HashSet::new();
        let first = follow::tail(&path, &follow::Seam::default(), &earlier, &|_| true, false, &|_| false).unwrap().unwrap();
        let shown: Vec<String> = first.transcript.events.iter().map(|event| serde_json::to_string(event).unwrap()).collect();
        assert!(shown.iter().any(|event| event.contains("bytes not shown")), "the large text is cut");
        // An event may show the cut text more than once (its words and their
        // preview); the line was 9 MiB.
        assert!(shown.iter().all(|event| event.len() < 2 << 20), "nothing of it is held whole");
        let seam = first.seam.expect("the seam moves past the large line");
        let seen: HashSet<String> = first.prefix_ids.iter().cloned().collect();
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all([row(5, "later"), row(6, "more words")].concat().as_bytes()).unwrap();
        let second = follow::tail(&path, &seam, &earlier, &|_| true, false, &|id: &str| seen.contains(id)).unwrap();
        let second = second.expect("read on from the seam, not whole again");
        let texts: Vec<String> = second.transcript.events.iter().map(|event| serde_json::to_string(event).unwrap()).collect();
        assert!(texts.iter().any(|event| event.contains("more words")));
        assert!(texts.iter().all(|event| !event.contains("bytes not shown")), "the large line is behind the seam");
    }

    #[test]
    fn a_structure_too_large_is_not_read() {
        let numbers: Vec<u32> = (0..3_000_000).collect();
        let line = serde_json::to_vec(&json!({"n": numbers})).unwrap();
        assert!(line.len() > MOST);
        assert!(cut(&line, 1 << 20).is_none());
    }
}
