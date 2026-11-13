//! The marks of how this machine is shared in its usage ledgers (7965 P08,
//! `usage::Entry::Shared`), as the copy of the ledgers reads them
//! (`activity`): each line goes where the mark in force as it was written
//! says, a body's or its account's.

use std::collections::BTreeMap;

/// What this machine was shared as, by hub id: the body it was on there
/// while shared whole, or none (`born_shared::now`).
pub(super) type Marked = BTreeMap<String, Option<String>>;

/// The sharing a usage ledger's line marks, if it is a mark
/// (`usage::Entry::Shared`).
pub(super) fn mark_of(line: &[u8]) -> Option<Marked> {
    if !line.windows(8).any(|window| window == b"\"shared\"") {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(line).ok()?;
    if value.get("kind").and_then(serde_json::Value::as_str) != Some("shared") {
        return None;
    }
    serde_json::from_value(value.get("hubs")?.clone()).ok()
}

/// The marks in force at `offset` of the day's file `file` at `path`: as
/// the standing kept them where this look stands, or read from the file's
/// start; none where nothing before it is marked.
pub(super) fn marks_at(at: &serde_json::Value, file: &str, path: &std::path::Path, offset: u64) -> Result<Option<Marked>, String> {
    if let Some(kept) = at["marks"].get(file) {
        return Ok(serde_json::from_value(kept.clone()).ok());
    }
    if offset == 0 {
        return Ok(None);
    }
    let bytes = {
        use std::io::Read as _;
        let opened = fastctx_platform::disk::open(path).map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        opened.take(offset).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
        bytes
    };
    Ok(marks_through(None, &bytes))
}

/// The marks in force after the lines `bytes`, from `marks` before them.
pub(super) fn marks_through(marks: Option<Marked>, bytes: &[u8]) -> Option<Marked> {
    bytes.split(|byte| *byte == b'\n').filter_map(mark_of).next_back().or(marks)
}

/// The lines at the start of `bytes` made under one sharing on the network
/// `hub`, `marks` in force before them: how long they are, and that sharing
/// — the body, or none — where the ledger marks one. A mark among them that
/// shares this network otherwise ends them, and begins the next piece.
pub(super) fn marked_run(mut marks: Option<Marked>, bytes: &[u8], hub: &str) -> (usize, Option<Option<String>>) {
    let on = |marks: &Option<Marked>| marks.as_ref().map(|marked| marked.get(hub).cloned().flatten());
    let (mut run, mut lines) = (0, false);
    let mut sharing = on(&marks);
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if let Some(marked) = mark_of(line) {
            let next = Some(marked);
            if lines && on(&next) != sharing {
                break;
            }
            marks = next;
            sharing = on(&marks);
        } else {
            lines = true;
        }
        run += line.len();
    }
    (run, sharing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(hub: &str, body: Option<&str>) -> String {
        format!("{}\n", serde_json::json!({ "kind": "shared", "at": "2026-11-13T00:00:00Z", "hubs": { hub: body } }))
    }

    #[test]
    fn a_run_ends_where_the_sharing_changes() {
        let line = "{\"kind\":\"cost\",\"who\":null}\n";
        let bytes = format!("{}{line}{line}{}{line}", mark("h", None), mark("h", Some("b")));
        let (run, sharing) = marked_run(None, bytes.as_bytes(), "h");
        assert_eq!(sharing, Some(None), "not shared as marked first");
        assert_eq!(&bytes[..run], format!("{}{line}{line}", mark("h", None)), "the next mark begins the next piece");
        let (rest, sharing) = marked_run(marks_through(None, &bytes.as_bytes()[..run]), &bytes.as_bytes()[run..], "h");
        assert_eq!(sharing, Some(Some("b".to_string())), "then the body's");
        assert_eq!(rest, bytes.len() - run);
    }

    #[test]
    fn a_mark_for_another_network_or_none_changes_nothing_here() {
        let line = "{\"kind\":\"cost\"}\n";
        let bytes = format!("{line}{}{line}", mark("other", Some("b")));
        let (run, sharing) = marked_run(None, bytes.as_bytes(), "h");
        assert_eq!(sharing, None, "nothing marked here before the first line: as before");
        assert_eq!(run, line.len(), "a mark that says this network is not shared ends a run marked nowhere");
        let (run, sharing) = marked_run(None, line.as_bytes(), "h");
        assert_eq!((run, sharing), (line.len(), None));
    }
}
