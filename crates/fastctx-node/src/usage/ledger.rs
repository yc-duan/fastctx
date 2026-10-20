//! Complete ledger rows and their in-memory indexes. A newline commits a row.
use super::recent::Charge;
use fastctx_types::access::HubAccount;
use fastctx_types::agents::LimitWindow;
use fastctx_types::usage::{KindsUsd, Tokens, Usd};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub(super) type Reading = (Timestamp, Vec<LimitWindow>);
pub(super) type Identity = (String, Option<String>);

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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Entry {
    Cost {
        at: Timestamp,
        who: Option<HubAccount>,
        model: String,
        usd: Usd,
    },
    Tokens {
        at: Timestamp,
        who: Option<HubAccount>,
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
        session: String,
        process: Option<String>,
        total: Usd,
        uses: Vec<Use>,
    },
    Reading {
        at: Timestamp,
        windows: Vec<LimitWindow>,
    },
    Summary {
        at: Timestamp,
        who: Option<HubAccount>,
        #[serde(flatten)]
        usage: Use,
    },
    Receipts {
        at: Timestamp,
        keys: Vec<String>,
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
            Self::Cost { at, .. }
            | Self::Tokens { at, .. }
            | Self::Spent { at, .. }
            | Self::Reading { at, .. }
            | Self::Summary { at, .. }
            | Self::Checkpoint { at, .. }
            | Self::Receipts { at, .. } => *at,
        }
    }
    pub fn uses(&self) -> Vec<(Option<HubAccount>, Use)> {
        match self {
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
            _ => Vec::new(),
        }
    }
}

#[derive(Default)]
pub(super) struct Book {
    valid: bool,
    pub totals: HashMap<Identity, (Timestamp, Usd)>,
    pub seen: HashSet<(Identity, Timestamp)>,
    pub modern: HashSet<String>,
    pub keys: HashSet<String>,
    pub reading: Option<Reading>,
    pub recent: Vec<Charge>,
    pub history: super::history::History,
}
impl Book {
    pub fn load(&mut self, path: &Path) -> io::Result<()> {
        if self.valid {
            return Ok(());
        }
        *self = Self::default();
        for file in days(path, Timestamp::MIN)? {
            let rows = read(&file)?;
            for row in &rows {
                self.apply(row);
            }
            self.history.insert(file, &rows);
        }
        self.valid = true;
        Ok(())
    }
    fn apply(&mut self, row: &Entry) {
        let now = Timestamp::now();
        let recent = now - SignedDuration::from_hours(8 * 24);
        let month = now - SignedDuration::from_hours(31 * 24);
        match row {
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
                self.seen.insert((identity.clone(), *at));
                if let Entry::Checkpoint { reports, .. } = row {
                    self.seen.extend(reports.iter().map(|at| (identity.clone(), *at)));
                }
            }
            Entry::Tokens { at, key: Some(key), .. } if *at >= month => {
                self.keys.insert(key.clone());
            }
            Entry::Receipts { at, keys } if *at >= month => {
                self.keys.extend(keys.iter().cloned());
            }
            Entry::Reading { at, windows } => {
                let mut merged = self.reading.as_ref().map_or_else(Vec::new, |(_, windows)| windows.clone());
                super::readings::merge(&mut merged, windows.clone());
                if self.reading.as_ref().is_none_or(|(_, old)| *old != merged) {
                    self.reading = Some((*at, merged));
                }
            }
            _ => {}
        }
        if row.at() >= recent {
            for (who, usage) in row.uses() {
                if let Some(usd) = usage.usd {
                    self.recent.push(Charge {
                        at: row.at(),
                        who,
                        fable: fastctx_hosts::claude::pricing::is_fable(&usage.model),
                        usd,
                    });
                }
            }
        }
        self.recent.retain(|charge| charge.at >= recent);
    }
    pub fn commit(&mut self, path: &Path, row: Entry) -> io::Result<()> {
        if let Entry::Tokens { key: Some(key), .. } = &row
            && self.keys.contains(key)
        {
            return Ok(());
        }
        // 2026-10-20: reload ambiguous writes from disk, not a half-updated index.
        self.valid = false;
        append(path, &row)?;
        self.apply(&row);
        self.history.invalidate(&day_file(path, row.at()));
        self.valid = true;
        Ok(())
    }
    pub fn invalidate(&mut self) {
        self.valid = false;
    }
}

pub(super) fn day_file(path: &Path, at: Timestamp) -> PathBuf {
    path.join(format!("{}.jsonl", at.strftime("%Y-%m-%d")))
}

fn append(path: &Path, row: &Entry) -> io::Result<()> {
    fastctx_platform::ensure_private_dir(path)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(day_file(path, row.at()))?;
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
    let read = match std::fs::read_dir(path) {
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
    let bytes = std::fs::read(path)?;
    let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let mut rows = Vec::new();
    for (line, bytes) in bytes[..end].split(|b| *b == b'\n').enumerate() {
        if bytes.is_empty() {
            continue;
        }
        rows.push(
            serde_json::from_slice(bytes)
                .map_err(|e| io::Error::other(format!("{} line {}: {e}", path.display(), line + 1)))?,
        );
    }
    Ok(rows)
}

pub(super) fn entries(path: &Path, since: Timestamp) -> io::Result<Vec<Entry>> {
    let mut rows = Vec::new();
    for path in days(path, since)? {
        rows.extend(read(&path)?.into_iter().filter(|row| row.at() >= since));
    }
    rows.sort_by_key(Entry::at);
    Ok(rows)
}
