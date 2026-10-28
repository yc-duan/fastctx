//! Folds old use into hourly per-person/model rows while retaining recovery metadata.
use super::ledger::{self, Entry, Identity, Use};
use crate::Node;
use fastctx_types::access::HubAccount;
use fastctx_types::usage::{KindsUsd, Tokens};
use jiff::{SignedDuration, Timestamp};
use std::collections::BTreeMap;
use std::io;

pub(crate) fn collect(node: &Node) -> io::Result<()> {
    let root = super::root(node);
    let ledgers = match std::fs::read_dir(root) {
        Ok(read) => read,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let now = Timestamp::now();
    let oldest = (now - SignedDuration::from_hours(24 * super::KEPT_DAYS)).strftime("%Y-%m-%d").to_string();
    let detail_since = now - SignedDuration::from_hours(24 * super::calls::DETAIL_DAYS);
    let fold_before = detail_since.strftime("%Y-%m-%d").to_string();
    for path in ledgers {
        let path = path?.path();
        if !path.is_dir() {
            continue;
        }
        super::with_book(node, &path, |book| {
            if let Some((since, account)) = book.accounts.last().cloned()
                && since.strftime("%Y-%m-%d").to_string() < oldest
            {
                book.commit(&path, Entry::Account { at: now, account })?;
            }
            let all = ledger::entries(&path, Timestamp::MIN)?;
            let week = super::readings::week(book.reading.as_ref(), now);
            let boundaries = super::report::boundaries(&super::report::spans(&all, week), week);
            for day in ledger::days(&path, Timestamp::MIN)? {
                let date = day.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
                if date < oldest.as_str() {
                    std::fs::remove_file(&day)?;
                    continue;
                }
                if date > fold_before.as_str() {
                    continue;
                }
                let (old, mut recent): (Vec<_>, Vec<_>) =
                    ledger::read(&day)?.into_iter().partition(|row| row.at() < detail_since);
                let mut rows = fold(&old, &boundaries, now);
                rows.append(&mut recent);
                rows.sort_by_key(Entry::at);
                let bytes = encode(&rows)?;
                if std::fs::read(&day)? != bytes {
                    fastctx_platform::publish(&day, &bytes, fastctx_platform::Permissions::Private)?;
                }
            }
            book.invalidate();
            book.load(&path)
        })?;
    }
    Ok(())
}

type HourUses = BTreeMap<(Timestamp, String, String, bool), (Option<HubAccount>, Use)>;

pub(super) fn fold(rows: &[Entry], boundaries: &[Timestamp], now: Timestamp) -> Vec<Entry> {
    let mut uses = HourUses::new();
    let mut checkpoints: BTreeMap<Identity, (Timestamp, fastctx_types::usage::Usd, Vec<Timestamp>)> = BTreeMap::new();
    let mut readings: BTreeMap<String, Entry> = BTreeMap::new();
    let mut periods: Vec<Timestamp> = Vec::new();
    let mut last = BTreeMap::new();
    let mut accounts = Vec::new();
    let mut meters = BTreeMap::new();
    let mut receipts = Vec::new();
    let mut receipts_at = Timestamp::MIN;
    let month = now - SignedDuration::from_hours(31 * 24);
    for row in rows {
        for (who, usage) in row.uses() {
            let bucket = if matches!(row, Entry::Summary { .. }) {
                row.usage_at()
            } else {
                super::report::bucket(row.usage_at(), boundaries)
            };
            let key = (
                bucket,
                serde_json::to_string(&who).expect("account serializes"),
                usage.model.clone(),
                usage.kinds.is_some(),
            );
            let (_, total) = uses.entry(key).or_insert_with(|| {
                (who, Use { model: usage.model.clone(), usd: None, tokens: Tokens::default(), kinds: None, call: None })
            });
            total.tokens.add(&usage.tokens);
            if let Some(usd) = usage.usd {
                total.usd = Some(total.usd.unwrap_or_default().saturating_add(usd));
                if let Some(kinds) = usage.kinds {
                    super::claude::add_kinds(total.kinds.get_or_insert_with(KindsUsd::default), kinds);
                }
            }
        }
        match row {
            Entry::Account { .. } => accounts.push(row.clone()),
            Entry::Meter { meter } => {
                let key = (meter.session.clone(), meter.process.clone());
                if let Some(receipt) = &meter.key {
                    receipts.push(receipt.clone());
                    receipts_at = receipts_at.max(meter.at);
                }
                if meters.get(&key).is_none_or(|old: &super::meter::Meter| old.at <= meter.at) {
                    let mut checkpoint = *meter.clone();
                    checkpoint.usage = None;
                    checkpoint.key = None;
                    meters.insert(key, checkpoint);
                }
            }
            Entry::Tokens { at, key: Some(key), .. } if *at >= month => {
                receipts.push(key.clone());
                receipts_at = receipts_at.max(*at);
            }
            Entry::Receipts { at, keys } if *at >= month => {
                receipts.extend(keys.iter().cloned());
                receipts_at = receipts_at.max(*at);
            }
            Entry::Spent { at, session, process, total, .. }
            | Entry::Checkpoint { at, session, process, total, .. }
                if *at >= month =>
            {
                let kept = checkpoints.entry((session.clone(), process.clone())).or_insert((*at, *total, Vec::new()));
                if *at >= kept.0 {
                    kept.0 = *at;
                    kept.1 = *total;
                }
                kept.2.push(*at);
                if let Entry::Checkpoint { reports, .. } = row {
                    kept.2.extend(reports);
                }
            }
            Entry::Reading { windows, account, .. } => {
                for window in windows.iter().filter(|w| w.minutes == Some(10_080)) {
                    let reset = window
                        .resets_at
                        .map(|reset| {
                            let canonical = periods.iter().copied().find(|old| super::readings::same(*old, reset));
                            let canonical = canonical.unwrap_or_else(|| {
                                periods.push(reset);
                                reset
                            });
                            canonical.to_string()
                        })
                        .unwrap_or_default();
                    let reset = format!("{}:{reset}", account.as_deref().unwrap_or("legacy"));
                    // The first sample fixes the reset's spelling within the two-minute
                    // tolerance. Dropping it would move historical weekly boundaries.
                    readings.entry(format!("first:{reset}")).or_insert_with(|| row.clone());
                    let replace = readings.get(&reset).is_none_or(|row| match row {
                        Entry::Reading { windows, .. } => {
                            windows
                                .iter()
                                .filter(|w| w.minutes == Some(10_080))
                                .map(|w| w.used_tenths)
                                .max()
                                .unwrap_or(0)
                                <= window.used_tenths
                        }
                        _ => true,
                    });
                    if replace {
                        readings.insert(reset, row.clone());
                    }
                }
                last.insert(account.clone(), row.clone());
            }
            _ => {}
        }
    }
    let mut folded: Vec<_> = uses
        .into_iter()
        .map(|((at, _, _, _), (who, usage))| Entry::Summary { at, who, with: Vec::new(), usage })
        .collect();
    folded.extend(checkpoints.into_iter().map(|((session, process), (at, total, mut reports))| {
        reports.sort();
        reports.dedup();
        Entry::Checkpoint { at, session, process, total, reports }
    }));
    if !receipts.is_empty() {
        receipts.sort();
        receipts.dedup();
        folded.push(Entry::Receipts { at: receipts_at, keys: receipts });
    }
    for (account, row) in last {
        readings.insert(format!("last:{}", account.as_deref().unwrap_or("legacy")), row);
    }
    folded.extend(accounts);
    folded.extend(meters.into_values().map(|meter| Entry::Meter { meter: Box::new(meter) }));
    let mut kept: Vec<_> = readings.into_values().collect();
    kept.sort_by_key(Entry::at);
    kept.dedup_by_key(|row| row.at());
    folded.extend(kept);
    folded.sort_by_key(Entry::at);
    folded
}

fn encode(rows: &[Entry]) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut bytes, row).map_err(io::Error::other)?;
        bytes.push(b'\n');
    }
    Ok(bytes)
}
