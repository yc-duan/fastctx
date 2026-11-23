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
    /// of its own beside the messages, the boundary `boundary` keeping
    /// `kept` — the messages, or the segment of `metadata` where given — and
    /// going on from the last of them, naming `anchor` the summary to come,
    /// then an attachment, stamped a moment before the boundary. When it
    /// was written.
    fn compact(&mut self, boundary: &str, anchor: &str, kept: &[String], metadata: Option<Value>) -> u32 {
        self.lines.push(json!({"type": "last-prompt", "lastPrompt": "words"}).to_string());
        self.clock += 1;
        let last = kept.last().cloned();
        let metadata = metadata.unwrap_or_else(|| {
            json!({"trigger": "auto", "preTokens": 880_000,
                   "preservedSegment": {"headUuid": kept.first(), "anchorUuid": anchor, "tailUuid": last},
                   "preservedMessages": {"anchorUuid": anchor, "uuids": kept, "allUuids": kept}})
        });
        let row = json!({"type": "system", "subtype": "compact_boundary", "uuid": boundary, "parentUuid": null, "logicalParentUuid": last,
                         "timestamp": stamp(self.clock, 500), "content": "Conversation compacted", "compactMetadata": metadata});
        self.lines.push(row.to_string());
        let attachment = format!("t-{boundary}");
        self.lines.push(json!({"type": "attachment", "uuid": attachment, "parentUuid": boundary, "timestamp": stamp(self.clock, 492), "attachment": {"type": "date", "date": "2026-11-23"}}).to_string());
        self.parent = Some(attachment);
        self.clock
    }

    /// `compact` as b1, naming s1; the summary written where `summary`.
    fn compaction(&mut self, kept: &[String], summary: bool) -> u32 {
        let clock = self.compact("b1", "s1", kept, None);
        if summary {
            self.summary(clock);
        }
        clock
    }

    /// The summary s1 of the compaction written at `clock`.
    fn summary(&mut self, clock: u32) {
        self.summary_as("s1", clock);
    }

    /// The summary `uuid` of a compaction written at `clock`.
    fn summary_as(&mut self, uuid: &str, clock: u32) {
        self.push(json!({"type": "user", "uuid": uuid, "timestamp": stamp(clock, 300), "isCompactSummary": true, "isVisibleInTranscriptOnly": true,
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
    begun_with(path, |_| {})
}

/// As `begun`, with what `earlier` writes before the second turn.
fn begun_with(path: &Path, earlier: impl FnOnce(&mut Record)) -> (Record, Seam, HashSet<String>, Timestamp) {
    let mut record = Record::default();
    record.words("u1");
    record.reply("a1");
    earlier(&mut record);
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

/// A summary no compaction past the seam names its anchor is read whole:
/// one before the seam may name it, as a whole reading finds (here one
/// that named it before its words were written), however the summary
/// stands below a compaction of the turn's own. A microcompaction before
/// the seam leaves the context as it was; a compaction there, its summary
/// not written, leaves it untold, which reads the record whole too.
#[test]
fn a_summary_no_compaction_past_the_seam_names_is_read_whole() {
    for subtype in ["microcompact_boundary", "compact_boundary"] {
        let dir = folder(&format!("summary-named-before-{subtype}"));
        let path = dir.join("record.jsonl");
        let (mut record, seam, before, said) = begun_with(&path, |record| {
            record.clock += 1;
            record.lines.push(
                json!({"type": "system", "subtype": subtype, "uuid": "b0", "parentUuid": null, "logicalParentUuid": "a1", "timestamp": stamp(record.clock, 500),
                       "compactMetadata": {"preservedMessages": {"anchorUuid": "s0", "uuids": ["a1"]}}})
                .to_string(),
            );
            record.parent = Some("b0".to_owned());
        });
        let accept = move |at: Timestamp| at <= said;
        let kept = record.round().to_vec();
        let compacted = record.compaction(&kept, false);
        record.summary_as("s0", compacted);
        record.write(&path);
        let read = tail(&path, &seam, &HashSet::new(), &accept, false, &|id| before.contains(id)).unwrap();
        if subtype == "microcompact_boundary" {
            assert_eq!(read.err(), Some(Restart::Unsupported("a compaction naming rows before the seam")));
        } else {
            assert!(matches!(read, Err(Restart::Unsupported(_))), "{subtype}: read past the seam");
        }
        let _ = fastctx_platform::disk::remove_dir_all(&dir);
    }
}

/// Past a place read through inside the turn, a compaction naming as its
/// anchor a summary before the place — one a whole reading takes from
/// where it stood and shows with this compaction — is read from the seam.
#[test]
fn a_compaction_naming_a_summary_before_the_place_is_read_from_the_seam() {
    let dir = folder("anchor-before-the-place");
    let path = dir.join("record.jsonl");
    let (mut record, seam, before, said) = begun(&path);
    let accept = move |at: Timestamp| at <= said;
    let kept = record.round().to_vec();
    let compacted = record.compact("b0", "s0", &kept, None);
    record.summary_as("s0", compacted);
    record.round();
    record.round();
    record.write(&path);
    let (judged, _, keys) = read_kept(&path, &seam, &before, None, &accept);
    let place = judged
        .unwrap_or_else(|why| panic!("read whole: {why}"))
        .read
        .expect("a place read through past the compaction");
    let keys: HashSet<String> = keys.into_iter().collect();
    assert!(keys.contains(&format!("{KEY_ROW}s0")), "the summary lies before the place");
    let kept = record.round().to_vec();
    record.compact("b1", "s0", &kept, None);
    record.write(&path);
    let on = read_kept(&path, &place, &before, Some(&keys), &|_| false).0;
    assert!(matches!(on, Err(Restart::Unsupported("a compaction naming rows before the seam"))), "{on:?}");
    let _ = fastctx_platform::disk::remove_dir_all(&dir);
}

/// A compaction keeping a segment whose head and tail lie past the seam but
/// a row between them, along their parents, before it is read whole.
#[test]
fn a_kept_segment_through_a_row_before_the_seam_is_read_whole() {
    let dir = folder("segment-through-before");
    let path = dir.join("record.jsonl");
    // A row before the seam whose parent is written after it.
    let (mut record, seam, before, said) = begun_with(&path, |record| {
        let at = record.at();
        record.lines.push(json!({"type": "assistant", "uuid": "between", "parentUuid": "head", "timestamp": at,
                                 "message": {"id": "m-between", "role": "assistant", "model": "x", "content": [{"type": "text", "text": "between"}]}}).to_string());
    });
    let accept = move |at: Timestamp| at <= said;
    let at = record.at();
    record.push(json!({"type": "assistant", "uuid": "head", "timestamp": at, "message": {"id": "m-head", "role": "assistant", "model": "x", "content": [{"type": "text", "text": "head"}]}}));
    let at = record.at();
    record.lines.push(json!({"type": "user", "uuid": "tail", "parentUuid": "between", "timestamp": at, "message": {"role": "user", "content": "tail"}}).to_string());
    record.parent = Some("tail".to_owned());
    let kept = ["head".to_owned(), "tail".to_owned()];
    let segment =
        json!({"trigger": "auto", "preservedSegment": {"headUuid": "head", "anchorUuid": "s1", "tailUuid": "tail"}});
    let compacted = record.compact("b1", "s1", &kept, Some(segment));
    record.summary(compacted);
    record.write(&path);
    let read = tail(&path, &seam, &HashSet::new(), &accept, false, &|id| before.contains(id)).unwrap();
    assert_eq!(read.err(), Some(Restart::Unsupported("a compaction naming rows before the seam")));
    let _ = fastctx_platform::disk::remove_dir_all(&dir);
}

/// Before the seam a compaction whose context could not be told — told so
/// by every reading while it is the last — then one inside the turn that is
/// whole: a whole reading tells only the last, so the record is read whole.
#[test]
fn a_compaction_past_one_whose_context_was_not_told_is_read_whole() {
    let dir = folder("context-untold");
    let path = dir.join("record.jsonl");
    let (mut record, seam, before, said) = begun_with(&path, |record| {
        let compacted = record.compact("b0", "s0", &["gone".to_owned()], None);
        record.summary_as("s0", compacted);
    });
    assert!(
        seam.problems.iter().any(|problem| problem.starts_with(super::super::CONTEXT_UNTOLD)),
        "{:?}",
        seam.problems
    );
    let accept = move |at: Timestamp| at <= said;
    let kept = record.round().to_vec();
    record.compaction(&kept, true);
    record.round();
    record.write(&path);
    let read = tail(&path, &seam, &HashSet::new(), &accept, false, &|id| before.contains(id)).unwrap();
    assert_eq!(
        read.err(),
        Some(Restart::Unsupported("a compaction past one before the seam whose context was not told"))
    );
    let _ = fastctx_platform::disk::remove_dir_all(&dir);
}
