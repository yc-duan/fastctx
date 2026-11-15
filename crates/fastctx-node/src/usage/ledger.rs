//! Complete ledger rows and their in-memory indexes. A newline commits a row.
use super::recent::Charge;
use fastctx_types::access::HubAccount;
use fastctx_types::agents::LimitWindow;
use fastctx_types::usage::{KindsUsd, Tokens, UsageReset, Usd};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub(super) type Reading = (Timestamp, Vec<LimitWindow>);
pub(super) type Identity = (String, Option<String>);

/// How far back the totals and receipts that tell a usage counted once go.
pub(super) const MONTH: SignedDuration = SignedDuration::from_hours(31 * 24);
/// How far back the spending that assistance limits weigh goes.
pub(super) const RECENT: SignedDuration = SignedDuration::from_hours(8 * 24);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Use {
    pub model: String,
    pub usd: Option<Usd>,
    pub tokens: Tokens,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<KindsUsd>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<super::calls::Detail>,
}

impl Use {
    /// The `index`th of `count` even shares of it, what does not divide
    /// going to the first ones.
    fn share(&self, index: u64, count: u64) -> Use {
        let part = |value: u64| value / count + u64::from(index < value % count);
        let usd = |usd: Usd| Usd(part(usd.0));
        Use {
            model: self.model.clone(),
            usd: self.usd.map(usd),
            tokens: Tokens {
                input: part(self.tokens.input),
                cache_read: part(self.tokens.cache_read),
                cache_write: part(self.tokens.cache_write),
                output: part(self.tokens.output),
                reasoning: part(self.tokens.reasoning),
            },
            kinds: self.kinds.as_ref().map(|kinds| KindsUsd {
                input: usd(kinds.input),
                cache_read: usd(kinds.cache_read),
                cache_write: usd(kinds.cache_write),
                output: usd(kinds.output),
            }),
            call: self.call.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Entry {
    Meter {
        #[serde(flatten)]
        meter: Box<super::meter::Meter>,
    },
    Cost {
        at: Timestamp,
        who: Option<HubAccount>,
        model: String,
        usd: Usd,
    },
    Tokens {
        at: Timestamp,
        who: Option<HubAccount>,
        /// The other accounts it was used for alike, each booked an even
        /// share with `who`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        with: Vec<HubAccount>,
        model: String,
        tokens: Tokens,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call: Option<super::calls::Detail>,
    },
    Spent {
        at: Timestamp,
        who: Option<HubAccount>,
        /// The other accounts it was used for alike, each booked an even
        /// share with `who`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        with: Vec<HubAccount>,
        session: String,
        process: Option<String>,
        total: Usd,
        uses: Vec<Use>,
    },
    Reading {
        at: Timestamp,
        windows: Vec<LimitWindow>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<String>,
        /// Conclusions committed atomically with the quota snapshot.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        resets: Vec<UsageReset>,
    },
    Account {
        at: Timestamp,
        account: String,
    },
    Summary {
        at: Timestamp,
        who: Option<HubAccount>,
        /// The other accounts it was used for alike, each booked an even
        /// share with `who`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        with: Vec<HubAccount>,
        #[serde(flatten)]
        usage: Use,
    },
    Receipts {
        at: Timestamp,
        keys: Vec<String>,
    },
    /// What this machine is shared as while the lines after it are written
    /// (7965 P08): by hub id, the body it is on there while shared whole, or
    /// none. Written before the first line of a day's file, and again before
    /// a line written while it is shared otherwise, so the copy of the ledger
    /// (`assets::activity`) sends each line where it went as it was written.
    Shared {
        at: Timestamp,
        hubs: std::collections::BTreeMap<String, Option<String>>,
    },
    /// Recovery metadata survives hourly folding until the 31-day replay horizon.
    Checkpoint {
        at: Timestamp,
        session: String,
        process: Option<String>,
        total: Usd,
        reports: Vec<Timestamp>,
    },
}

impl Entry {
    pub fn at(&self) -> Timestamp {
        match self {
            Self::Meter { meter } => meter.at,
            Self::Cost { at, .. }
            | Self::Tokens { at, .. }
            | Self::Spent { at, .. }
            | Self::Reading { at, .. }
            | Self::Account { at, .. }
            | Self::Summary { at, .. }
            | Self::Checkpoint { at, .. }
            | Self::Shared { at, .. }
            | Self::Receipts { at, .. } => *at,
        }
    }
    pub fn usage_at(&self) -> Timestamp {
        match self {
            Self::Meter { meter } => {
                meter.usage.as_ref().and_then(|usage| usage.call.as_ref()).map_or(meter.at, |call| call.timing.ended_at)
            }
            _ => self.at(),
        }
    }
    pub fn uses(&self) -> Vec<(Option<HubAccount>, Use)> {
        let mut uses: Vec<_> = match self {
            Self::Cost { who, model, usd, .. } => vec![(
                who.clone(),
                Use { model: model.clone(), usd: Some(*usd), tokens: Tokens::default(), kinds: None, call: None },
            )],
            Self::Tokens { who, model, tokens, call, .. } => {
                vec![(
                    who.clone(),
                    Use { model: model.clone(), usd: None, tokens: *tokens, kinds: None, call: call.clone() },
                )]
            }
            Self::Spent { who, uses, .. } => uses.iter().map(|usage| (who.clone(), usage.clone())).collect(),
            Self::Summary { who, usage, .. } => vec![(who.clone(), usage.clone())],
            Self::Meter { meter } => meter.usage.iter().map(|usage| (meter.who.clone(), usage.clone())).collect(),
            _ => Vec::new(),
        };
        // Used for several accounts at once, each is booked an even share
        // (`world-usage.md`, 替谁).
        let with = match self {
            Self::Tokens { with, .. } | Self::Spent { with, .. } | Self::Summary { with, .. } => with.as_slice(),
            Self::Meter { meter } => meter.with.as_slice(),
            _ => &[],
        };
        if !with.is_empty() {
            let count = with.len() as u64 + 1;
            uses = uses
                .into_iter()
                .flat_map(|(who, usage)| {
                    std::iter::once(who)
                        .chain(with.iter().cloned().map(Some))
                        .enumerate()
                        .map(|(index, who)| (who, usage.share(index as u64, count)))
                        .collect::<Vec<_>>()
                })
                .collect();
        }
        for (_, usage) in &mut uses {
            if usage
                .call
                .as_ref()
                .is_some_and(|call| call.source == fastctx_types::usage_calls::CallSource::Background && !call.observed)
            {
                usage.model.clear();
            }
        }
        uses
    }
}

#[derive(Default)]
pub(super) struct Book {
    valid: bool,
    /// Where each call of the days a page may show lies (`super::calls`).
    pub calls: super::calls::Index,
    pub totals: HashMap<Identity, (Timestamp, Usd)>,
    pub meters: HashMap<Identity, super::meter::Meter>,
    /// The reports counted, each with when the row that counted it was
    /// written, which is how long it is kept.
    pub seen: HashMap<(Identity, Timestamp), Timestamp>,
    pub modern: HashSet<String>,
    /// The keys of usage booked already, each with when the row that booked
    /// it was written; none for one kept for as long as the ledger is.
    pub keys: HashMap<String, Option<Timestamp>>,
    pub reading: Option<Reading>,
    pub readings: HashMap<Option<String>, Reading>,
    /// Pending detection and source observations, separate for each account.
    pub lowered: HashMap<Option<String>, super::readings::Lowered>,
    pub resets: HashMap<Option<String>, Vec<UsageReset>>,
    /// Whether future early weekly resets are detected automatically.
    pub follow: bool,
    pub accounts: Vec<(Timestamp, String)>,
    pub recent: Vec<Charge>,
    pub history: super::history::History,
    /// The last day file the ledger's checkpoint holds (`super::checkpoint`).
    sealed: Option<PathBuf>,
    /// What this machine is shared as now, given before each write
    /// (`super::with_book`); none where no write marks it.
    pub sharing: Option<std::collections::BTreeMap<String, Option<String>>>,
    /// The last mark of each day's file written to, as read or written.
    marked: HashMap<PathBuf, Option<std::collections::BTreeMap<String, Option<String>>>>,
}
impl Book {
    pub fn load(&mut self, path: &Path) -> io::Result<()> {
        if self.valid {
            return Ok(());
        }
        self.load_at(path, Timestamp::now())
    }

    /// Reads the ledger as it stands at `now`: what its checkpoint holds of
    /// the days that no longer change, and the days after them row by row,
    /// keeping a checkpoint through the last day that no longer changes
    /// where the one read holds less. The days' own index is made when a
    /// report reads them (`super::history`).
    pub(super) fn load_at(&mut self, path: &Path, now: Timestamp) -> io::Result<()> {
        *self = Self { follow: super::follows_clearings(path), ..Self::default() };
        let files = days(path, Timestamp::MIN)?;
        let open = super::checkpoint::first_open(path, now);
        let mut from = 0;
        if let Some((state, through)) = super::checkpoint::restore(path, &files) {
            super::checkpoint::adopt(self, state, now);
            from = files.partition_point(|file| *file <= through);
            self.sealed = Some(through);
        }
        for (index, file) in files.iter().enumerate().skip(from) {
            for row in &read(file)? {
                self.apply_at(row, now);
            }
            if *file < open && files.get(index + 1).is_none_or(|next| *next >= open) {
                // A checkpoint is a cache of the day files: one that cannot
                // be kept leaves the next load reading them all, as it did.
                if super::checkpoint::save(path, self, &files[..=index]).is_ok() {
                    self.sealed = Some(file.clone());
                }
            }
        }
        self.valid = true;
        Ok(())
    }

    /// Keeps the report `at` of `identity` as counted by a row written `row`.
    fn saw(&mut self, identity: &Identity, at: Timestamp, row: Timestamp) {
        let kept = self.seen.entry((identity.clone(), at)).or_insert(row);
        *kept = (*kept).max(row);
    }

    /// Keeps `key` as booked by a row written `at`, or for good.
    fn booked(&mut self, key: &str, at: Option<Timestamp>) {
        match self.keys.get_mut(key) {
            Some(kept) => {
                *kept = match (*kept, at) {
                    (Some(kept), Some(at)) => Some(kept.max(at)),
                    _ => None,
                };
            }
            None => {
                self.keys.insert(key.to_string(), at);
            }
        }
    }

    fn apply(&mut self, row: &Entry) {
        self.apply_at(row, Timestamp::now());
    }

    pub(super) fn apply_at(&mut self, row: &Entry, now: Timestamp) {
        let recent = now - RECENT;
        let month = now - MONTH;
        match row {
            Entry::Meter { meter } => {
                let identity = (meter.session.clone(), Some(meter.process.clone()));
                if let Some(key) = &meter.key {
                    self.booked(key, None);
                }
                self.modern.insert(meter.session.clone());
                if self.meters.get(&identity).is_none_or(|old| old.at <= meter.at) {
                    self.meters.insert(identity, *meter.clone());
                }
            }
            Entry::Spent { at, session, process, total, .. }
            | Entry::Checkpoint { at, session, process, total, .. }
                if *at >= month =>
            {
                let identity = (session.clone(), process.clone());
                let entry = self.totals.entry(identity.clone()).or_insert((*at, *total));
                if *at >= entry.0 {
                    *entry = (*at, *total);
                }
                if process.is_some() {
                    self.modern.insert(session.clone());
                }
                self.saw(&identity, *at, *at);
                if let Entry::Checkpoint { reports, .. } = row {
                    for report in reports {
                        self.saw(&identity, *report, *at);
                    }
                }
            }
            Entry::Tokens { at, key: Some(key), .. } if *at >= month || key.starts_with("terminal:") => {
                self.booked(key, (!key.starts_with("terminal:")).then_some(*at));
            }
            Entry::Receipts { at, keys } => {
                for key in keys.iter().filter(|key| *at >= month || key.starts_with("terminal:")) {
                    self.booked(key, (!key.starts_with("terminal:")).then_some(*at));
                }
            }
            Entry::Account { at, account } => {
                if self.accounts.is_empty() {
                    // The first known owner claims legacy readings once, durably.
                    if let Some(legacy) = self.readings.remove(&None) {
                        self.readings.insert(Some(account.clone()), legacy);
                    }
                    if let Some(legacy) = self.lowered.remove(&None) {
                        self.lowered.insert(Some(account.clone()), legacy);
                    }
                    if let Some(legacy) = self.resets.remove(&None) {
                        self.resets.insert(Some(account.clone()), legacy);
                    }
                }
                self.accounts.push((*at, account.clone()));
                self.accounts.sort_by_key(|(at, _)| *at);
                self.reading = self.readings.get(&self.account_at(Timestamp::MAX)).cloned();
            }
            Entry::Reading { at, windows, account, resets } => {
                let account = account.clone().or_else(|| self.account_at(*at));
                let old = self.readings.get(&account);
                let mut merged = old.map_or_else(Vec::new, |(_, windows)| windows.clone());
                if old.is_none_or(|(seen, _)| *seen <= *at) {
                    let history = self.resets.entry(account.clone()).or_default();
                    super::resets::merge_recorded(&mut merged, windows, *at, resets, history);
                    self.readings.insert(account, (*at, merged));
                }
                self.reading = self.readings.get(&self.account_at(Timestamp::MAX)).cloned();
            }
            _ => {}
        }
        if row.usage_at() >= recent {
            for (who, usage) in row.uses() {
                if let Some(usd) = usage.usd {
                    self.recent.push(Charge {
                        at: row.usage_at(),
                        who,
                        fable: fastctx_hosts::claude::pricing::is_fable(&usage.model)
                            && usage.call.as_ref().is_none_or(|call| call.observed),
                        usd,
                    });
                }
            }
        }
        self.recent.retain(|charge| charge.at >= recent);
    }
    pub fn reset_history(&self) -> Vec<(Option<String>, UsageReset)> {
        let mut resets: Vec<_> = self
            .resets
            .iter()
            .flat_map(|(account, resets)| resets.iter().map(move |reset| (account.clone(), reset.clone())))
            .collect();
        resets.sort_by_key(|(_, reset)| reset.confirmed_at);
        resets
    }
    pub fn balances(&self) -> Vec<Entry> {
        self.meters.values().flat_map(super::meter::Meter::balances).collect()
    }
    pub fn account_at(&self, at: Timestamp) -> Option<String> {
        self.accounts
            .iter()
            .rev()
            .find(|(since, _)| *since <= at)
            .or_else(|| self.accounts.first())
            .map(|(_, account)| account.clone())
    }
    pub fn commit(&mut self, path: &Path, row: Entry) -> io::Result<()> {
        if let Entry::Tokens { key: Some(key), .. } = &row
            && self.keys.contains_key(key)
        {
            return Ok(());
        }
        // A row for a day the checkpoint holds makes it what the day was.
        if self.sealed.as_ref().is_some_and(|sealed| day_file(path, row.usage_at()) <= *sealed) {
            super::checkpoint::discard(path)?;
            self.sealed = None;
        }
        // Reload ambiguous writes from disk, not a half-updated index.
        self.valid = false;
        // Marked first where the day's file was last marked otherwise, or
        // not at all (7965 P08).
        if let Some(sharing) = self.sharing.clone() {
            let file = day_file(path, row.usage_at());
            let marked = match self.marked.get(&file) {
                Some(marked) => marked.clone(),
                None => last_mark(&file)?,
            };
            if marked.as_ref() != Some(&sharing) {
                append(path, &Entry::Shared { at: row.usage_at(), hubs: sharing.clone() })?;
            }
            self.marked.insert(file, Some(sharing));
        }
        append(path, &row)?;
        self.apply(&row);
        self.history.invalidate(&day_file(path, row.usage_at()));
        self.valid = true;
        Ok(())
    }
}

/// The last mark in the day's file `file`; none when it holds none, or is
/// not there.
fn last_mark(file: &Path) -> io::Result<Option<std::collections::BTreeMap<String, Option<String>>>> {
    let bytes = match fastctx_platform::disk::read(file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    // Whole lines alone: a last line its newline did not reach was never
    // written, as the ledger reads it (codex-847f57fd #18).
    Ok(bytes
        .split_inclusive(|byte| *byte == b'\n')
        .filter(|line| line.ends_with(b"\n") && line.windows(8).any(|window| window == b"\"shared\""))
        .filter_map(|line| match serde_json::from_slice::<Entry>(line) {
            Ok(Entry::Shared { hubs, .. }) => Some(hubs),
            _ => None,
        })
        .next_back())
}

pub(super) fn day_file(path: &Path, at: Timestamp) -> PathBuf {
    path.join(format!("{}.jsonl", at.strftime("%Y-%m-%d")))
}

fn append(path: &Path, row: &Entry) -> io::Result<()> {
    fastctx_platform::ensure_private_dir(path)?;
    let mut file = fastctx_platform::disk::open_with(
        std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true),
        day_file(path, row.usage_at()),
    )?;
    let len = file.metadata()?.len();
    if len > 0 {
        let mut end = len;
        let complete = loop {
            let start = end.saturating_sub(8192);
            file.seek(SeekFrom::Start(start))?;
            let mut tail = vec![0; (end - start) as usize];
            file.read_exact(&mut tail)?;
            if let Some(index) = tail.iter().rposition(|b| *b == b'\n') {
                break start + index as u64 + 1;
            }
            if start == 0 {
                break 0;
            }
            end = start;
        };
        if complete != len {
            file.set_len(complete)?;
        }
    }
    // The login's book lock serializes writes, including torn-tail repair.
    file.seek(SeekFrom::End(0))?;
    let mut bytes = serde_json::to_vec(row).map_err(io::Error::other)?;
    bytes.push(b'\n');
    file.write_all(&bytes)?;
    file.sync_data()
}

pub(super) fn days(path: &Path, since: Timestamp) -> io::Result<Vec<PathBuf>> {
    let first = since.strftime("%Y-%m-%d").to_string();
    let read = match fastctx_platform::disk::read_dir(path) {
        Ok(read) => read,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut files = Vec::new();
    for entry in read {
        let path = entry?.path();
        if path.extension().is_some_and(|s| s == "jsonl")
            && path.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s >= first.as_str())
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

pub(super) fn read(path: &Path) -> io::Result<Vec<Entry>> {
    super::quarantine::read(path)
}
