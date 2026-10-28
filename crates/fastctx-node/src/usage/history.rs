//! Completed days are indexed once. Prefix sums retain exact moving-day and reset boundaries.
use super::ledger::{self, Entry, Reading, Use};
use fastctx_types::access::HubAccount;
use fastctx_types::usage::{KindsUsd, Tokens, Usd};
use jiff::{SignedDuration, Timestamp};
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(super) struct History {
    days: HashMap<PathBuf, Day>,
}
struct Group {
    who: Option<HubAccount>,
    points: Vec<(Timestamp, Use)>,
}
pub(super) struct Day {
    readings: Vec<Entry>,
    groups: Vec<Group>,
    folded: Vec<Timestamp>,
}
impl Day {
    pub fn new(rows: &[Entry]) -> Self {
        let mut groups: BTreeMap<String, Group> = BTreeMap::new();
        for row in rows {
            for (who, mut usage) in row.uses() {
                usage.call = None;
                let key = serde_json::to_string(&(&who, &usage.model, usage.usd.is_some(), usage.kinds.is_some()))
                    .expect("usage key serializes");
                groups
                    .entry(key)
                    .or_insert_with(|| Group { who, points: Vec::new() })
                    .points
                    .push((row.usage_at(), usage));
            }
        }
        for group in groups.values_mut() {
            group.points.sort_by_key(|(at, _)| *at);
            let mut sum = Use { model: String::new(), usd: None, tokens: Tokens::default(), kinds: None, call: None };
            for (_, usage) in &mut group.points {
                sum.tokens.add(&usage.tokens);
                if let Some(usd) = usage.usd {
                    sum.usd = Some(sum.usd.unwrap_or_default().saturating_add(usd));
                }
                if let Some(kinds) = usage.kinds {
                    super::claude::add_kinds(sum.kinds.get_or_insert_with(Default::default), kinds);
                }
                usage.tokens = sum.tokens;
                usage.usd = sum.usd;
                usage.kinds = sum.kinds;
            }
        }
        Self {
            readings: rows
                .iter()
                .filter(|r| matches!(r, Entry::Reading { .. } | Entry::Account { .. }))
                .cloned()
                .collect(),
            groups: groups.into_values().collect(),
            folded: rows.iter().filter(|row| matches!(row, Entry::Summary { .. })).map(Entry::at).collect(),
        }
    }
    fn summaries(&self, cuts: &[Timestamp], periods: &[Timestamp], into: &mut Vec<Entry>) {
        for group in &self.groups {
            let Some((first, _)) = group.points.first() else { continue };
            let last = group.points.last().expect("nonempty").0;
            let hour = first.as_second().div_euclid(3600) * 3600;
            let end = last.as_second().div_euclid(3600) * 3600 + 3600;
            let mut edges: Vec<Timestamp> =
                (hour..=end).step_by(3600).map(|s| Timestamp::from_second(s).expect("existing day")).collect();
            edges.extend(cuts.iter().copied().filter(|at| at.as_second() > hour && at.as_second() < end));
            edges.extend(self.folded.iter().copied().filter(|at| at.as_second() >= hour && at.as_second() < end));
            for second in (hour..end).step_by(3600) {
                let end_of_hour = Timestamp::from_second(second + 3599).expect("existing hour");
                let cut = super::report::bucket(end_of_hour, periods);
                if cut.as_second() > second {
                    edges.push(cut);
                }
            }
            edges.sort();
            edges.dedup();
            for edge in edges.windows(2) {
                let low = group.points.partition_point(|(at, _)| *at < edge[0]);
                let high = group.points.partition_point(|(at, _)| *at < edge[1]);
                if low == high {
                    continue;
                }
                let mut usage = group.points[high - 1].1.clone();
                if low > 0 {
                    subtract(&mut usage, &group.points[low - 1].1);
                }
                into.push(Entry::Summary { at: edge[0], who: group.who.clone(), with: Vec::new(), usage });
            }
        }
    }
}

fn subtract(to: &mut Use, from: &Use) {
    to.tokens.input -= from.tokens.input;
    to.tokens.cache_read -= from.tokens.cache_read;
    to.tokens.cache_write -= from.tokens.cache_write;
    to.tokens.output -= from.tokens.output;
    to.tokens.reasoning -= from.tokens.reasoning;
    if let Some(usd) = to.usd {
        to.usd = Some(usd.saturating_sub(from.usd.unwrap_or(Usd::ZERO)));
    }
    if let (Some(a), Some(b)) = (&mut to.kinds, from.kinds) {
        *a = KindsUsd {
            input: a.input.saturating_sub(b.input),
            cache_read: a.cache_read.saturating_sub(b.cache_read),
            cache_write: a.cache_write.saturating_sub(b.cache_write),
            output: a.output.saturating_sub(b.output),
        };
    }
}

impl History {
    pub fn insert(&mut self, file: PathBuf, rows: &[Entry]) {
        self.days.insert(file, Day::new(rows));
    }
    pub fn invalidate(&mut self, file: &Path) {
        self.days.remove(file);
    }
    pub fn entries(
        &mut self,
        path: &Path,
        since: Timestamp,
        now: Timestamp,
        reading: Option<&Reading>,
    ) -> io::Result<Vec<Entry>> {
        let files = ledger::days(path, since)?;
        self.days.retain(|file, _| files.contains(file));
        let today = ledger::day_file(path, now);
        let mut current = None;
        for file in &files {
            if *file == today {
                current = Some(Day::new(&ledger::read(file)?));
            } else if !self.days.contains_key(file) {
                self.insert(file.clone(), &ledger::read(file)?);
            }
        }
        let days: Vec<&Day> = files
            .iter()
            .filter(|file| **file != today)
            .filter_map(|file| self.days.get(file))
            .chain(current.as_ref())
            .collect();
        let mut entries: Vec<Entry> = days.iter().flat_map(|day| day.readings.iter().cloned()).collect();
        let week = super::readings::week(reading, now);
        let periods = super::report::boundaries(&super::report::spans(&entries, week), week);
        let mut cuts = periods.clone();
        cuts.extend([now - SignedDuration::from_hours(24), week.start, week.end.max(now)]);
        for day in days {
            day.summaries(&cuts, &periods, &mut entries);
        }
        entries.sort_by_key(Entry::at);
        Ok(entries)
    }
}
