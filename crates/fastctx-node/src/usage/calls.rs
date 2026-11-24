//! Request details are local financial records; conversation ownership is a separate check.
use super::ledger;
use super::tail::{Read, Tail};
use crate::Node;
use crate::admission::seen::Looker;
use fastctx_types::ResidentId;
use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::usage::Usd;
use fastctx_types::usage_calls::{
    CallCursor, CallOutcome, CallPhase, CallSource, CallTiming, UsageCall, UsageCallsPage, UsageCallsQuery, UsageCallsSum,
};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{self, BufRead, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub(super) const DETAIL_DAYS: i64 = 30;

/// Ownership remains available after the conversation itself is deleted.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Detail {
    /// None in a body's copy of a call made for a conversation an account
    /// was assisted in: that conversation is the account's alone
    /// (the body's records scope, as earlier releases wrote it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<ResidentId>,
    pub acting: Acting,
    pub turn: Option<String>,
    pub source: CallSource,
    pub agent: Option<String>,
    /// Older rows have none; leaving it out keeps their stored identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub timing: CallTiming,
    pub outcome: CallOutcome,
    pub phase: CallPhase,
    pub observed: bool,
}

/// The calls of each day's file, by where each one's row lies in it: kept in
/// the login's book (`ledger::Book`) and read on from where it stopped as the
/// file grows (`tail`), so that a page reads its own hundred rows rather than
/// the month.
#[derive(Default)]
pub(super) struct Index {
    days: HashMap<PathBuf, Day>,
    who: Names<Option<HubAccount>>,
    models: Names<String>,
    conversations: Names<Option<ResidentId>>,
    turns: Names<Option<String>>,
    acting: Vec<Acting>,
}

#[derive(Default)]
struct Day {
    /// How far its file has been read.
    tail: Tail,
    calls: Vec<Call>,
}

/// One call, with what a query selects it by; the rest stays in its line.
struct Call {
    at: Timestamp,
    key: [u8; 32],
    /// Where its row's line begins in the file.
    line: u64,
    item: u32,
    who: u32,
    model: u32,
    acting: u32,
    conversation: u32,
    turn: u32,
    source: CallSource,
    outcome: CallOutcome,
    /// Whether its model may be shown (`row`).
    named: bool,
    /// What it cost and the tokens it took, for what a query's rows add up
    /// to; its tokens count only where they are shown (`row`).
    usd: u64,
    input: u64,
    output: u64,
    observed: bool,
    fable: bool,
}

/// Values met many times, each kept once.
struct Names<T> {
    values: Vec<T>,
    ids: HashMap<T, u32>,
}

impl<T> Default for Names<T> {
    fn default() -> Self {
        Self { values: Vec::new(), ids: HashMap::new() }
    }
}

impl<T: Clone + Eq + std::hash::Hash> Names<T> {
    fn id(&mut self, value: T) -> u32 {
        if let Some(id) = self.ids.get(&value) {
            return *id;
        }
        let id = u32::try_from(self.values.len()).expect("fewer names than that");
        self.values.push(value.clone());
        self.ids.insert(value, id);
        id
    }

    fn get(&self, id: u32) -> &T {
        &self.values[id as usize]
    }
}

impl Index {
    /// Brings the day's calls up to what its file holds now.
    fn refresh(&mut self, file: &Path) -> io::Result<()> {
        let mut day = self.days.remove(file).unwrap_or_default();
        let rows = match day.tail.read(file)? {
            Read::Same => {
                self.days.insert(file.to_path_buf(), day);
                return Ok(());
            }
            Read::Gone => return Ok(()),
            Read::Grew(rows) => rows,
            Read::Whole(rows) => {
                day.calls.clear();
                rows
            }
        };
        for (line, entry) in &rows {
            self.add(entry, *line, &mut day.calls)?;
        }
        self.days.insert(file.to_path_buf(), day);
        Ok(())
    }

    fn add(&mut self, entry: &ledger::Entry, line: u64, calls: &mut Vec<Call>) -> io::Result<()> {
        let uses = entry.uses();
        if uses.iter().all(|(_, usage)| usage.call.is_none()) {
            return Ok(());
        }
        let identity = identity(entry)?;
        let at = entry.usage_at();
        for (item, (who, usage)) in uses.into_iter().enumerate() {
            let Some(detail) = usage.call else { continue };
            let acting = match self.acting.iter().position(|known| *known == detail.acting) {
                Some(known) => known,
                None => {
                    self.acting.push(detail.acting);
                    self.acting.len() - 1
                }
            };
            let fable = fastctx_hosts::claude::pricing::is_fable(&usage.model);
            calls.push(Call {
                at,
                key: key(&identity, item),
                line,
                item: u32::try_from(item).expect("fewer uses than that"),
                who: self.who.id(who),
                model: self.models.id(usage.model),
                acting: u32::try_from(acting).expect("fewer than that"),
                conversation: self.conversations.id(detail.conversation),
                turn: self.turns.id(detail.turn),
                source: detail.source,
                outcome: detail.outcome,
                named: detail.source != CallSource::Background || detail.observed,
                usd: usage.usd.map_or(0, |usd| usd.0),
                input: usage.tokens.input + usage.tokens.cache_read + usage.tokens.cache_write,
                output: usage.tokens.output,
                observed: detail.observed,
                fable,
            });
        }
        Ok(())
    }
}

/// Row contents remain stable when expired neighbors are folded away.
fn identity(entry: &ledger::Entry) -> io::Result<[u8; 32]> {
    Ok(Sha256::digest(serde_json::to_vec(entry).map_err(io::Error::other)?).into())
}

fn key(identity: &[u8; 32], item: usize) -> [u8; 32] {
    let mut key = Sha256::new();
    key.update(identity);
    key.update((item as u64).to_be_bytes());
    key.finalize().into()
}

fn hex(key: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    key.iter().fold(String::with_capacity(64), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// What a query selects a row by, as the row shows it.
struct Shown<'a> {
    at: Timestamp,
    key: &'a [u8; 32],
    who: Option<&'a HubAccount>,
    model: Option<&'a str>,
    source: CallSource,
    outcome: CallOutcome,
    conversation: Option<&'a ResidentId>,
    turn: Option<&'a str>,
}

impl Shown<'_> {
    /// Filtering after redaction cannot be used to discover another conversation.
    fn selected(&self, query: &UsageCallsQuery) -> bool {
        !(query.person.as_ref().is_some_and(|p| p.account.as_ref() != self.who && !(p.with_machine && self.who.is_none()))
            || query.model.as_ref().is_some_and(|m| self.model != Some(m.as_str()))
            || query.source.is_some_and(|s| s != self.source)
            || query.outcome.is_some_and(|s| s != self.outcome)
            || query.conversation.as_ref().is_some_and(|c| self.conversation != Some(c))
            || query.turn.as_ref().is_some_and(|t| self.turn != Some(t.as_str()))
            || query.before.as_ref().is_some_and(|before| {
                self.at.cmp(&before.at).then_with(|| hex(self.key).cmp(&before.key)).is_ge()
            }))
    }
}

/// A row a query selected: one kept in the index, read from its line when
/// its page is made, or one the book holds apart, made already.
enum Found {
    Kept { file: usize, line: u64, item: u32 },
    Made(Box<UsageCall>),
}

fn row(cursor: CallCursor, who: Option<HubAccount>, usage: ledger::Use, detail: Detail, owns: bool) -> UsageCall {
    let named = detail.source != CallSource::Background || detail.observed;
    UsageCall {
        cursor,
        who,
        model: named.then_some(usage.model),
        effort: detail.effort.filter(|_| named),
        tokens: detail.observed.then_some(usage.tokens),
        usd: usage.usd,
        source: detail.source,
        agent: detail.agent,
        timing: detail.timing,
        outcome: detail.outcome,
        phase: detail.phase,
        conversation: detail.conversation.filter(|_| owns),
        turn: if owns { detail.turn } else { None },
    }
}

/// Admission has already selected this login. `only` restricts rows, while
/// `looker` independently controls the two private conversation identifiers.
pub(crate) fn calls(
    node: &Node,
    agent: &str,
    dir: &Path,
    only: Option<&HubAccount>,
    looker: Looker<'_>,
    query: &UsageCallsQuery,
) -> io::Result<UsageCallsPage> {
    let path = super::ledger(node, agent, dir);
    super::with_book(node, &path, |book| calls_in(node, book, &path, only, looker, query))
}

/// The page of calls `query` asks for of the ledger at `path`, loaded as
/// `book`: this machine's own, or one put back together from copies.
pub(super) fn calls_in(
    node: &Node,
    book: &mut super::ledger::Book,
    path: &Path,
    only: Option<&HubAccount>,
    looker: Looker<'_>,
    query: &UsageCallsQuery,
) -> io::Result<UsageCallsPage> {
    let kept = Timestamp::now() - SignedDuration::from_hours(24 * DETAIL_DAYS);
    let since = query.from.map_or(kept, |from| from.max(kept));
    // An entry is filed under its own day, so the days after the cursor, or
    // after the end of the range asked for, are not read.
    let day = |at: &Timestamp| at.strftime("%Y-%m-%d").to_string();
    let last = [query.before.as_ref().map(|before| day(&before.at)), query.until.as_ref().map(day)]
        .into_iter()
        .flatten()
        .min();
    let numbered = query.page.is_some();
    let size = query.page_size();

    let in_range = |at: Timestamp| at >= since && query.until.is_none_or(|until| at < until);
    {
        let balances = book.balances();
        let index = &mut book.calls;
        // The days no query reaches any longer are let go, and with them the
        // names only they used: the index is read again whole (about once a
        // day), so that it holds the window's and no more.
        let oldest = day(&kept);
        if index.days.keys().any(|file| file.file_stem().and_then(|s| s.to_str()).is_none_or(|s| s < oldest.as_str())) {
            *index = Index::default();
        }
        let mut files = ledger::days(path, since)?;
        files.reverse();
        if let Some(last) = &last {
            files.retain(|file| file.file_stem().and_then(|s| s.to_str()).is_none_or(|day| day <= last.as_str()));
        }
        let mut owns: HashMap<u32, bool> = HashMap::new();
        let mut found: Vec<(Timestamp, [u8; 32], Found)> = Vec::new();
        // What the rows selected add up to, where the query counts them.
        let mut sum = UsageCallsSum::default();
        for (at_file, file) in files.iter().enumerate() {
            index.refresh(file)?;
            if let Some(kept) = index.days.get(file) {
                for call in &kept.calls {
                    if !in_range(call.at) {
                        continue;
                    }
                    let who = index.who.get(call.who).as_ref();
                    if only.is_some_and(|only| who != Some(only)) {
                        continue;
                    }
                    let sees = *owns
                        .entry(call.acting)
                        .or_insert_with(|| looker.sees(node, &index.acting[call.acting as usize]));
                    let shown = Shown {
                        at: call.at,
                        key: &call.key,
                        who,
                        model: call.named.then(|| index.models.get(call.model).as_str()),
                        source: call.source,
                        outcome: call.outcome,
                        conversation: index.conversations.get(call.conversation).as_ref().filter(|_| sees),
                        turn: index.turns.get(call.turn).as_deref().filter(|_| sees),
                    };
                    if shown.selected(query) {
                        tally(&mut sum, call.usd, call.fable, call.observed.then_some((call.input, call.output)));
                        found.push((call.at, call.key, Found::Kept { file: at_file, line: call.line, item: call.item }));
                    }
                }
            }
            for entry in balances.iter().filter(|row| ledger::day_file(path, row.at()) == *file) {
                if !in_range(entry.usage_at()) {
                    continue;
                }
                let mut digest = None;
                for (item, (who, usage)) in entry.uses().into_iter().enumerate() {
                    if only.is_some_and(|only| who.as_ref() != Some(only)) {
                        continue;
                    }
                    let Some(detail) = usage.call.clone() else { continue };
                    let made = match digest {
                        Some(made) => made,
                        None => {
                            let made = identity(entry)?;
                            digest = Some(made);
                            made
                        }
                    };
                    let key = key(&made, item);
                    let owns = looker.sees(node, &detail.acting);
                    let made = row(CallCursor { at: entry.usage_at(), key: hex(&key) }, who, usage, detail, owns);
                    let shown = Shown {
                        at: made.cursor.at,
                        key: &key,
                        who: made.who.as_ref(),
                        model: made.model.as_deref(),
                        source: made.source,
                        outcome: made.outcome,
                        conversation: made.conversation.as_ref(),
                        turn: made.turn.as_deref(),
                    };
                    if shown.selected(query) {
                        let tokens = made.tokens.as_ref().map(|tokens| (tokens.input + tokens.cache_read + tokens.cache_write, tokens.output));
                        tally(&mut sum, made.usd.map_or(0, |usd| usd.0), made.model.as_deref().is_some_and(fastctx_hosts::claude::pricing::is_fable), tokens);
                        found.push((made.cursor.at, key, Found::Made(Box::new(made))));
                    }
                }
            }
            // Paged by a cursor, the days older than a page's worth are left unread.
            if !numbered && found.len() > size {
                break;
            }
        }
        found.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
        let total = found.len();
        let start = query.page.map_or(0, |page| (page as usize).saturating_mul(size).min(total));
        let more = total - start > size;
        let mut rows = Vec::new();
        let mut opened: HashMap<usize, io::BufReader<std::fs::File>> = HashMap::new();
        for (at, key, found) in found.into_iter().skip(start).take(size) {
            rows.push(match found {
                Found::Made(made) => *made,
                Found::Kept { file, line, item } => {
                    let reader = match opened.entry(file) {
                        std::collections::hash_map::Entry::Occupied(reader) => reader.into_mut(),
                        std::collections::hash_map::Entry::Vacant(vacant) => {
                            vacant.insert(io::BufReader::new(fastctx_platform::disk::open(&files[file])?))
                        }
                    };
                    reader.seek(SeekFrom::Start(line))?;
                    let mut bytes = Vec::new();
                    reader.read_until(b'\n', &mut bytes)?;
                    let entry: ledger::Entry = serde_json::from_slice(bytes.trim_ascii_end()).map_err(io::Error::other)?;
                    let changed = || io::Error::other(format!("{} changed while it was read", files[file].display()));
                    let (who, usage) = entry.uses().into_iter().nth(item as usize).ok_or_else(changed)?;
                    let detail = usage.call.clone().ok_or_else(changed)?;
                    let owns = looker.sees(node, &detail.acting);
                    row(CallCursor { at, key: hex(&key) }, who, usage, detail, owns)
                }
            });
        }
        let next = more.then(|| rows.last().expect("full page").cursor.clone());
        Ok(UsageCallsPage {
            rows,
            next,
            total: numbered.then_some(total as u64),
            size: query.size.map(|_| size as u32),
            sum: numbered.then_some(sum),
        })
    }
}

/// Adds one selected row to what the rows add up to.
fn tally(sum: &mut UsageCallsSum, usd: u64, fable: bool, tokens: Option<(u64, u64)>) {
    sum.usd = sum.usd.saturating_add(Usd(usd));
    if fable {
        sum.fable_usd = sum.fable_usd.saturating_add(Usd(usd));
    }
    if let Some((input, output)) = tokens {
        sum.input = sum.input.saturating_add(input);
        sum.output = sum.output.saturating_add(output);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastctx_types::usage::Tokens;
    use std::io::Write as _;

    fn tokens(at: Timestamp, turn: &str) -> ledger::Entry {
        ledger::Entry::Tokens {
            at,
            who: None,
            with: Vec::new(),
            model: "model".into(),
            tokens: Tokens::default(),
            key: None,
            call: Some(Detail {
                conversation: None,
                acting: Acting::Here,
                turn: Some(turn.into()),
                source: CallSource::Main,
                agent: None,
                effort: None,
                timing: CallTiming { started_at: None, first_at: None, ended_at: at, approximate: false },
                outcome: CallOutcome::Completed,
                phase: CallPhase::Finished,
                observed: true,
            }),
        }
    }

    fn append(file: &Path, entries: &[ledger::Entry], tail: &[u8]) {
        let mut opened =
            fastctx_platform::disk::open_with(std::fs::OpenOptions::new().create(true).append(true), file).unwrap();
        for entry in entries {
            let mut bytes = serde_json::to_vec(entry).unwrap();
            bytes.push(b'\n');
            opened.write_all(&bytes).unwrap();
        }
        opened.write_all(tail).unwrap();
    }

    fn turns(index: &Index, file: &Path) -> Vec<String> {
        index.days[file].calls.iter().map(|call| index.turns.get(call.turn).clone().unwrap()).collect()
    }

    #[test]
    fn a_days_calls_are_read_on_as_its_file_grows_and_again_when_it_is_another() {
        let directory = std::env::temp_dir().join(format!("fastctx-calls-index-{}", std::process::id()));
        let _ = fastctx_platform::disk::remove_dir_all(&directory);
        fastctx_platform::disk::create_dir_all(&directory).unwrap();
        let file = directory.join("2026-11-15.jsonl");
        let at = Timestamp::now();
        // A last line still being written is not read yet.
        append(&file, &[tokens(at, "a"), tokens(at, "b")], br#"{"kind":"tok"#);
        let mut index = Index::default();
        index.refresh(&file).unwrap();
        assert_eq!(turns(&index, &file), ["a", "b"]);
        let second = index.days[&file].calls[1].line;
        let bytes = fastctx_platform::disk::read(&file).unwrap();
        let first_line = bytes.iter().position(|b| *b == b'\n').unwrap() as u64 + 1;
        assert_eq!((index.days[&file].calls[0].line, second), (0, first_line));
        // As the ledger appends: the torn tail cut, a row written after it.
        let whole = fastctx_platform::disk::read(&file).unwrap();
        let read = whole.iter().rposition(|b| *b == b'\n').unwrap() as u64 + 1;
        fastctx_platform::disk::open_with(std::fs::OpenOptions::new().write(true), &file).unwrap().set_len(read).unwrap();
        append(&file, &[tokens(at, "c")], b"");
        index.refresh(&file).unwrap();
        assert_eq!(turns(&index, &file), ["a", "b", "c"]);
        // The key is the one a row of the whole file was always given.
        let identity = identity(&tokens(at, "c")).unwrap();
        assert_eq!(index.days[&file].calls[2].key, key(&identity, 0));
        // Another file in its place, shorter or longer, is read whole.
        fastctx_platform::disk::remove_file(&file).unwrap();
        append(&file, &[tokens(at, "z")], b"");
        index.refresh(&file).unwrap();
        assert_eq!(turns(&index, &file), ["z"]);
        fastctx_platform::disk::remove_file(&file).unwrap();
        append(&file, &[tokens(at, "p"), tokens(at, "q"), tokens(at, "r"), tokens(at, "s")], b"");
        index.refresh(&file).unwrap();
        assert_eq!(turns(&index, &file), ["p", "q", "r", "s"]);
        // A line that is no row is set aside as the ledger sets it aside.
        append(&file, &[], b"not a row\n");
        append(&file, &[tokens(at, "t")], b"");
        index.refresh(&file).unwrap();
        assert_eq!(turns(&index, &file), ["p", "q", "r", "s", "t"]);
        assert_eq!(super::super::quarantine::count(&directory).unwrap(), 1);
        // Gone, it holds nothing.
        fastctx_platform::disk::remove_file(&file).unwrap();
        index.refresh(&file).unwrap();
        assert!(!index.days.contains_key(&file));
        let _ = fastctx_platform::disk::remove_dir_all(&directory);
    }
}
