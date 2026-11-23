//! A compaction inside a turn, read past the turn's seam, is shown as a
//! whole reading shows it: Claude Code 2.1.322 keeps the last few messages
//! before it, which the turn holds. One naming a row before where the
//! record is read from is read whole.

use super::*;
use serde_json::json;
use std::path::PathBuf;

fn stamp(second: u32, milli: u32) -> String {
    format!("2026-11-23T{:02}:{:02}:{:02}.{milli:03}Z", 10 + second / 3600, second / 60 % 60, second % 60)
}

/// A record written a row at a time, each the child of the row before, as
/// Claude Code writes one.
#[derive(Clone, Default)]
struct Record {
    lines: Vec<String>,
    clock: u32,
    parent: Option<String>,
    rounds: usize,
}

impl Record {
    fn at(&mut self) -> String {
        self.clock += 1;
        stamp(self.clock, 0)
    }

    fn push(&mut self, mut row: Value) {
        row["parentUuid"] = json!(self.parent);
        self.parent = row["uuid"].as_str().map(str::to_owned);
        self.lines.push(row.to_string());
    }

    /// Where the next row starts.
    fn end(&self) -> u64 {
        self.lines.iter().map(|line| line.len() as u64 + 1).sum()
    }

    fn words(&mut self, uuid: &str) {
        let at = self.at();
        self.push(json!({"type": "user", "uuid": uuid, "timestamp": at, "message": {"role": "user", "content": format!("words {uuid}")}}));
    }

    fn reply(&mut self, uuid: &str) {
        let at = self.at();
        self.push(json!({"type": "assistant", "uuid": uuid, "timestamp": at, "message": {"id": format!("m-{uuid}"), "role": "assistant", "model": "x", "content": [{"type": "text", "text": "done"}]}}));
    }

    /// A round of work: the model's words and its call, rows of one message,
    /// then the call's answer. Their UUIDs.
    fn round(&mut self) -> [String; 3] {
        self.rounds += 1;
        let n = self.rounds;
        let (words, call, answer, message, tool) =
            (format!("w{n}"), format!("c{n}"), format!("r{n}"), format!("m{n}"), format!("k{n}"));
        let at = self.at();
        self.push(json!({"type": "assistant", "uuid": words, "timestamp": at, "message": {"id": message, "role": "assistant", "model": "x", "content": [{"type": "text", "text": "next"}]}}));
        let at = self.at();
        self.push(json!({"type": "assistant", "uuid": call, "timestamp": at,
                         "message": {"id": message, "role": "assistant", "model": "x", "content": [{"type": "tool_use", "id": tool, "name": "Bash", "input": {"command": "ls"}}]}}));
        let at = self.at();
        self.push(json!({"type": "user", "uuid": answer, "timestamp": at, "sourceToolAssistantUUID": call,
                         "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool, "content": "out"}]}}));
        [words, call, answer]
    }

    /// A compaction as Claude Code 2.1.322 writes one inside a turn: a row
    /// of its own beside the messages, the boundary keeping `kept` and
    /// going on from the last of them, an attachment, then — where
    /// `summary` — the summary it names as its anchor, these two stamped a
    /// moment before the boundary. When it was written.
    fn compaction(&mut self, kept: &[String], summary: bool) -> u32 {
        self.lines.push(json!({"type": "last-prompt", "lastPrompt": "words"}).to_string());
        self.clock += 1;
        let last = kept.last().cloned();
        let boundary = json!({"type": "system", "subtype": "compact_boundary", "uuid": "b1", "parentUuid": null, "logicalParentUuid": last,
            "timestamp": stamp(self.clock, 500), "content": "Conversation compacted",
            "compactMetadata": {"trigger": "auto", "preTokens": 880_000,
                "preservedSegment": {"headUuid": kept.first(), "anchorUuid": "s1", "tailUuid": last},
                "preservedMessages": {"anchorUuid": "s1", "uuids": kept, "allUuids": kept}}});
        self.lines.push(boundary.to_string());
        self.lines.push(json!({"type": "attachment", "uuid": "t1", "parentUuid": "b1", "timestamp": stamp(self.clock, 492), "attachment": {"type": "date", "date": "2026-11-23"}}).to_string());
        self.parent = Some("t1".to_owned());
        if summary {
            self.summary(self.clock);
        }
        self.clock
    }

    /// The summary of the compaction written at `clock`.
    fn summary(&mut self, clock: u32) {
        self.push(json!({"type": "user", "uuid": "s1", "timestamp": stamp(clock, 300), "isCompactSummary": true, "isVisibleInTranscriptOnly": true,
                         "message": {"role": "user", "content": "This session is being continued from a previous conversation."}}));
    }

    fn write(&self, path: &Path) {
        fastctx_platform::disk::write(path, self.lines.join("\n") + "\n").unwrap();
    }
}

/// A turn of words and a reply, then a second turn of three rounds of work:
/// the record, and its seam at the second turn's words as a reading from
/// its start places it, with the record ids before it.
fn begun(path: &Path) -> (Record, Seam, HashSet<String>, Timestamp) {
    let mut record = Record::default();
    record.words("u1");
    record.reply("a1");
    let words = record.end();
    record.words("u2");
    let said: Timestamp = stamp(record.clock, 0).parse().unwrap();
    for _ in 0..3 {
        record.round();
    }
    record.write(path);
    let accept = move |at: Timestamp| at <= said;
    let first = tail(path, &Seam::default(), &HashSet::new(), &accept, false, &|_| false).unwrap().unwrap();
    let seam = first.seam.expect("a seam");
    assert_eq!(seam.offset, words, "the seam is at the second turn's words");
    (record, seam, first.prefix_ids.into_iter().collect(), said)
}

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fastctx-follow-{name}-{}", std::process::id()));
    let _ = fastctx_platform::disk::remove_dir_all(&dir);
    fastctx_platform::disk::create_dir_all(&dir).unwrap();
    dir
}

/// The record read past `seam` into a store, a place inside the turn read
/// through: what it found, its events, and the keys the rows the place
/// moved past leave. `held`: read on from a place whose keys these are.
fn read_kept(
    path: &Path,
    seam: &Seam,
    before: &HashSet<String>,
    held: Option<&HashSet<String>>,
    accept: &dyn Fn(Timestamp) -> bool,
) -> (Result<Tail, Restart>, Vec<SessionEvent>, Vec<String>) {
    let mut rows = HeldRows::default();
    let (mut events, mut passed) = (Vec::new(), Vec::new());
    let among = |keys: &[String]| -> io::Result<HashSet<String>> {
        Ok(keys.iter().filter(|key| held.is_some_and(|held| held.contains(*key))).cloned().collect())
    };
    let seen = |ids: &[String]| -> io::Result<HashSet<String>> {
        Ok(ids
            .iter()
            .filter(|id| before.contains(*id) || held.is_some_and(|held| held.contains(&format!("{KEY_ROW}{id}"))))
            .cloned()
            .collect())
    };
    let mut past = |key: &str| -> io::Result<()> {
        passed.push(key.to_owned());
        Ok(())
    };
    let read = tail_kept(
        path,
        seam,
        &|_| Ok(false),
        accept,
        false,
        &seen,
        TailStore {
            rows: &mut rows,
            events: &mut |event| {
                events.push(event);
                Ok(())
            },
            moved_past: &mut |_| Ok(()),
            asked: &[],
            read_on: held.map(|_| &among as Before<'_>),
            read_past: Some(&mut past as &mut dyn FnMut(&str) -> io::Result<()>),
        },
    )
    .unwrap();
    (read, events, passed)
}

/// What a whole reading of the record at `path` shows from `since` on, and
/// what it could not make out.
fn whole(path: &Path, since: Timestamp) -> (Value, Vec<String>) {
    let read = tail(path, &Seam::default(), &HashSet::new(), &|_| false, false, &|_| false).unwrap().unwrap();
    let shown: Vec<&SessionEvent> = read.transcript.events.iter().filter(|event| event.at >= since).collect();
    (serde_json::to_value(shown).unwrap(), read.transcript.problems)
}

/// Compacted inside the turn — before its summary is written and after,
/// the work going on — the record is read past the turn's seam, into a
/// store or held, and shows what a whole reading shows of the rows from
/// there on; the place read through lies past the compaction.
#[test]
fn a_compaction_inside_the_turn_is_read_past_the_seam_as_a_whole_reading_shows_it() {
    let dir = folder("compaction-in-the-turn");
    let path = dir.join("record.jsonl");
    let (mut record, seam, before, said) = begun(&path);
    let accept = move |at: Timestamp| at <= said;
    record.round();
    let kept = record.round().to_vec();
    let compacted = record.compaction(&kept, false);
    let read_alike = |record: &Record| {
        record.write(&path);
        let held = tail(&path, &seam, &HashSet::new(), &accept, false, &|id| before.contains(id)).unwrap();
        let held = held.unwrap_or_else(|why| panic!("read whole: {why}"));
        let (wanted, problems) = whole(&path, said);
        assert_eq!(serde_json::to_value(&held.transcript.events).unwrap(), wanted);
        assert_eq!(held.transcript.problems, problems);
        let (kept, events, _) = read_kept(&path, &seam, &before, None, &accept);
        let kept = kept.unwrap_or_else(|why| panic!("read whole: {why}"));
        assert_eq!(serde_json::to_value(&events).unwrap(), wanted);
        kept
    };
    // The boundary written, its summary not yet.
    read_alike(&record);
    record.summary(compacted);
    read_alike(&record);
    record.round();
    let last = record.end();
    record.round();
    let kept = read_alike(&record);
    let place = kept.read.expect("a place read through inside the turn");
    assert_eq!(place.offset, last, "the place is the last answer's start, past the compaction");
    let _ = fastctx_platform::disk::remove_dir_all(&dir);
}

/// A compaction keeping a message of the turn before is read whole from the
/// turn's seam; so is one past a place read through inside the turn that
/// keeps a message before it. One keeping only what lies past the place is
/// read on from there.
#[test]
fn a_compaction_naming_a_row_before_where_it_is_read_from_is_read_whole() {
    let dir = folder("compaction-before");
    let path = dir.join("record.jsonl");
    let (mut record, seam, before, said) = begun(&path);
    let accept = move |at: Timestamp| at <= said;
    let earlier = record.clone();
    let kept = ["a1".to_owned(), "u2".to_owned()];
    record.compaction(&kept, true);
    record.round();
    record.write(&path);
    let read = tail(&path, &seam, &HashSet::new(), &accept, false, &|id| before.contains(id)).unwrap();
    assert_eq!(read.err(), Some(Restart::Unsupported("a compaction naming rows before the seam")));
    assert!(matches!(
        read_kept(&path, &seam, &before, None, &accept).0,
        Err(Restart::Unsupported("a compaction naming rows before the seam"))
    ));

    // Read on from the place a reading from the seam read through.
    let mut record = earlier;
    record.write(&path);
    let (judged, _, keys) = read_kept(&path, &seam, &before, None, &accept);
    let place = judged.unwrap().read.expect("a place read through");
    let keys: HashSet<String> = keys.into_iter().collect();
    let [_, call, answer] = record.round();
    // Keeping an answer before the place: read from the seam again.
    let mut reaching = record.clone();
    reaching.compaction(&["r2".to_owned(), call.clone(), answer.clone()], true);
    reaching.write(&path);
    let on = read_kept(&path, &place, &before, Some(&keys), &|_| false).0;
    assert!(matches!(on, Err(Restart::Unsupported("a compaction naming rows before the seam"))), "{on:?}");
    // Keeping only what lies past it: read on from there.
    record.compaction(&[call, answer], true);
    record.round();
    record.write(&path);
    let on = read_kept(&path, &place, &before, Some(&keys), &|_| false).0;
    assert!(on.is_ok(), "{on:?}");
    let _ = fastctx_platform::disk::remove_dir_all(&dir);
}
