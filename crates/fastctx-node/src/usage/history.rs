//! Completed days are indexed once. Prefix sums retain exact moving-day and reset boundaries.
use super::ledger::{self, Entry, Reading, Use};
use super::tail::{Read, Tail};
use fastctx_types::access::HubAccount;
use fastctx_types::usage::{KindsUsd, Tokens, Usd};
use jiff::{SignedDuration, Timestamp};
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(super) struct History {
    days: HashMap<PathBuf, Day>,
    /// Today's, read on as its file grows rather than again each report
    /// (`tail`): the day still being written.
    today: Option<(PathBuf, Tail, Day)>,
}
struct Group {
    who: Option<HubAccount>,
    /// Each point with the sum of the group's use through it, in time order.
    points: Vec<(Timestamp, Use)>,
}
#[derive(Default)]
pub(super) struct Day {
    readings: Vec<Entry>,
    /// By what is summed together; in this order the summaries are made.
    groups: BTreeMap<String, Group>,
    folded: Vec<Timestamp>,
}
impl Day {
    pub fn new(rows: &[Entry]) -> Self {
        let mut day = Self::default();
        day.extend(rows);
        day
    }

    /// Adds rows written after those it holds: the day as `new` would make
    /// it of them all. A group's new points come after its old ones nearly
    /// always and are summed on from them; one that does not is taken back
    /// to each point's own use and summed again in order.
    fn extend<'a>(&mut self, rows: impl IntoIterator<Item = &'a Entry>) {
        // Each group grown, with how many points it held before.
        let mut grown: BTreeMap<String, usize> = BTreeMap::new();
        for row in rows {
            for (who, mut usage) in row.uses() {
                usage.call = None;
                let key = serde_json::to_string(&(&who, &usage.model, usage.usd.is_some(), usage.kinds.is_some()))
                    .expect("usage key serializes");
                let group = self.groups.entry(key.clone()).or_insert_with(|| Group { who, points: Vec::new() });
                grown.entry(key).or_insert(group.points.len());
                group.points.push((row.usage_at(), usage));
            }
            if matches!(row, Entry::Reading { .. } | Entry::Account { .. }) {
                self.readings.push(row.clone());
            }
            if matches!(row, Entry::Summary { .. }) {
                self.folded.push(row.at());
            }
        }
        for (key, held) in grown {
            let group = self.groups.get_mut(&key).expect("a group grown");
            let points = &mut group.points;
            let in_order = points[held..].windows(2).all(|pair| pair[0].0 <= pair[1].0)
                && (held == 0 || points[held - 1].0 <= points[held].0);
            if in_order {
                sum_from(points, held);
            } else {
                for index in (1..held).rev() {
                    let (before, after) = points.split_at_mut(index);
                    subtract(&mut after[0].1, &before[index - 1].1);
                }
                points.sort_by_key(|(at, _)| *at);
                sum_from(points, 0);
            }
        }
    }
    fn summaries(&self, cuts: &[Timestamp], periods: &[Timestamp], into: &mut Vec<Entry>) {
        for group in self.groups.values() {
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

/// Sums each point of `points` from `from` on, on top of the sum through the
/// point before it: each point's own use becomes the group's through it.
fn sum_from(points: &mut [(Timestamp, Use)], from: usize) {
    let mut sum = match from {
        0 => Use { model: String::new(), usd: None, tokens: Tokens::default(), kinds: None, call: None },
        _ => points[from - 1].1.clone(),
    };
    for (_, usage) in &mut points[from..] {
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
        resets: &[(Option<String>, fastctx_types::usage::UsageReset)],
    ) -> io::Result<Vec<Entry>> {
        let files = ledger::days(path, since)?;
        self.days.retain(|file, _| files.contains(file));
        let today = ledger::day_file(path, now);
        let mut current = None;
        for file in &files {
            if *file == today {
                if self.today.as_ref().is_none_or(|(kept, _, _)| *kept != today) {
                    self.today = Some((today.clone(), Tail::default(), Day::default()));
                }
                let (_, tail, day) = self.today.as_mut().expect("today's day kept");
                match tail.read(file)? {
                    Read::Same => {}
                    Read::Grew(rows) => day.extend(rows.iter().map(|(_, row)| row)),
                    Read::Whole(rows) => {
                        *day = Day::default();
                        day.extend(rows.iter().map(|(_, row)| row));
                    }
                    Read::Gone => *day = Day::default(),
                }
                current = Some(());
            } else if !self.days.contains_key(file) {
                self.insert(file.clone(), &ledger::read(file)?);
            }
        }
        let today_day = current.and(self.today.as_ref()).map(|(_, _, day)| day);
        let days: Vec<&Day> = files
            .iter()
            .filter(|file| **file != today)
            .filter_map(|file| self.days.get(file))
            .chain(today_day)
            .collect();
        let mut entries: Vec<Entry> = days.iter().flat_map(|day| day.readings.iter().cloned()).collect();
        let week = super::readings::week(reading, now);
        let periods = super::report::boundaries(&super::report::spans(&entries, week, resets), week);
        let mut cuts = periods.clone();
        cuts.extend([now - SignedDuration::from_hours(24), week.start, week.end.max(now)]);
        for day in days {
            day.summaries(&cuts, &periods, &mut entries);
        }
        entries.sort_by_key(Entry::at);
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows of two models, two people and dollars or tokens, their moments
    /// out of order as a ledger's rows can be.
    fn rows(count: usize) -> Vec<Entry> {
        let base = Timestamp::from_second(1_800_000_000).unwrap();
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let who = |n: u64| {
            n.is_multiple_of(3).then(|| HubAccount {
                hub: "w-4ecf5e28c4da2e66".parse().unwrap(),
                account: format!("a{}", n % 2).parse().unwrap(),
            })
        };
        (0..count)
            .map(|_| {
                let at = base + SignedDuration::from_secs((next() % 86_400) as i64);
                let model = if next().is_multiple_of(2) { "one" } else { "two" }.to_string();
                if next().is_multiple_of(4) {
                    Entry::Cost { at, who: who(next()), model, usd: Usd(next() % 1000) }
                } else {
                    let tokens = Tokens { input: next() % 5000, output: next() % 900, ..Tokens::default() };
                    Entry::Tokens { at, who: who(next()), with: Vec::new(), model, tokens, key: None, call: None }
                }
            })
            .collect()
    }

    fn drawn(day: &Day) -> String {
        serde_json::to_string(
            &day.groups.iter().map(|(key, group)| (key, &group.who, &group.points)).collect::<Vec<_>>(),
        )
        .unwrap()
    }

    #[test]
    fn a_day_read_on_is_the_day_read_whole() {
        let all = rows(600);
        let whole = Day::new(&all);
        for split in [0, 1, 37, 300, 599, 600] {
            let mut read_on = Day::new(&all[..split]);
            read_on.extend(&all[split..]);
            assert_eq!(drawn(&read_on), drawn(&whole), "split at {split}");
        }
        // In order, as a day is nearly always written: summed on, not again.
        let mut sorted = all.clone();
        sorted.sort_by_key(Entry::usage_at);
        let mut read_on = Day::new(&sorted[..250]);
        read_on.extend(&sorted[250..]);
        assert_eq!(drawn(&read_on), drawn(&Day::new(&sorted)));
    }
}
