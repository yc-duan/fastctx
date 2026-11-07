//! Weekly reset evidence and durable boundaries, independent of the detector's switch.
use fastctx_types::agents::LimitWindow;
use fastctx_types::usage::{UsageReset, UsageResetSource};
use jiff::{SignedDuration, Timestamp};
use std::collections::HashMap;

pub(super) const WEEK_MINUTES: u32 = 10_080;
const FULL: u32 = 20;
const RISE: u32 = 20;
const CONFIRM_AFTER: SignedDuration = SignedDuration::from_secs(5 * 60);
const SOURCES: usize = 65_536;

#[derive(Default)]
pub(super) struct Detector {
    last: HashMap<String, (Timestamp, u32)>,
    candidate: Option<Candidate>,
}

struct Candidate {
    since: Timestamp,
    from: u32,
    source: String,
}

impl Detector {
    /// Cached snapshots have no vendor observation identifier. Only a changed
    /// figure from a previously seen source is evidence; equal repetitions,
    /// even after successful requests, cannot prove that its quota was refreshed.
    pub fn observe(
        &mut self,
        old: &mut LimitWindow,
        incoming: &LimitWindow,
        at: Timestamp,
        source: &str,
        enabled: bool,
    ) -> Option<UsageReset> {
        if self.last.len() >= SOURCES && !self.last.contains_key(source) {
            self.last.clear();
            self.candidate = None;
        }
        let before = self.last.get(source).copied();
        if before.is_some_and(|(seen, _)| at <= seen) {
            return None;
        }
        self.last.insert(source.to_string(), (at, incoming.used_tenths));
        let changed = before.is_some_and(|(_, value)| value != incoming.used_tenths);
        if !enabled {
            self.candidate = None;
        }
        if before.is_some_and(|(_, value)| value == incoming.used_tenths) {
            return None;
        }
        // A newly resumed session can still carry the figure from before a
        // confirmed reset. It must change before it raises that reset's figure.
        if before.is_none() && old.since.is_some() {
            return None;
        }
        if incoming.used_tenths >= old.used_tenths {
            old.used_tenths = incoming.used_tenths;
        }
        if !changed || !enabled {
            return None;
        }
        if incoming.used_tenths > FULL {
            self.candidate = None;
            return None;
        }
        if self
            .candidate
            .as_ref()
            .is_some_and(|pending| pending.source == source && at.duration_since(pending.since) >= CONFIRM_AFTER)
        {
            let pending = self.candidate.take().expect("candidate checked");
            let reset = UsageReset {
                window: old.name.clone(),
                since: pending.since,
                confirmed_at: at,
                resets_at: old.resets_at.expect("weekly period checked"),
                before_used_tenths: Some(pending.from),
                used_tenths: incoming.used_tenths,
                source: UsageResetSource::Automatic,
            };
            old.since = Some(reset.since);
            old.used_tenths = incoming.used_tenths;
            // Other sources must join the new accounting interval afresh.
            self.last.retain(|known, _| known == source);
            return Some(reset);
        }
        if self.candidate.is_none() && old.used_tenths >= incoming.used_tenths.saturating_add(RISE) {
            self.candidate = Some(Candidate { since: at, from: old.used_tenths, source: source.to_string() });
        }
        None
    }
}

pub(super) fn valid(reset: &UsageReset, window: &LimitWindow, at: Timestamp) -> bool {
    window.minutes == Some(WEEK_MINUTES)
        && window.name == reset.window
        && window.resets_at.is_some_and(|end| super::readings::same(end, reset.resets_at))
        && if reset.source == UsageResetSource::Scheduled {
            reset.since == reset.resets_at - SignedDuration::from_secs(super::WEEK_SECONDS)
        } else {
            reset.since > reset.resets_at - SignedDuration::from_secs(super::WEEK_SECONDS)
        }
        && reset.since < reset.resets_at
        && reset.since <= reset.confirmed_at
        && reset.confirmed_at <= at
}

/// Replays written conclusions without running the detector or consulting its
/// switch. Legacy near-full resets remain labelled as older records, not as
/// confirmations by the new detector. Small old dips are not promoted.
pub(super) fn merge_recorded(
    known: &mut Vec<LimitWindow>,
    windows: &[LimitWindow],
    at: Timestamp,
    written: &[UsageReset],
    history: &mut Vec<UsageReset>,
) {
    for incoming in windows {
        let old = known.iter().position(|old| old.name == incoming.name && old.minutes == incoming.minutes);
        if old.is_some_and(|index| {
            matches!((known[index].resets_at, incoming.resets_at), (Some(a), Some(b)) if b < a && !super::readings::same(a, b))
        }) {
            continue;
        }
        let previous = old.map(|index| &known[index]);
        let same_period = previous.is_some_and(
            |old| matches!((old.resets_at, incoming.resets_at), (Some(a), Some(b)) if super::readings::same(a, b)),
        );
        let mut accepted: Vec<UsageReset> =
            written.iter().filter(|reset| valid(reset, incoming, at)).cloned().collect();
        if let Some(since) = incoming.since
            && incoming.used_tenths <= FULL
            && previous
                .filter(|_| same_period)
                .is_some_and(|old| old.used_tenths >= incoming.used_tenths.saturating_add(RISE))
            && !history.iter().any(|reset| reset.window == incoming.name && reset.since == since)
            && !accepted.iter().any(|reset| reset.since == since)
            && let Some(end) = incoming.resets_at
        {
            let reset = UsageReset {
                window: incoming.name.clone(),
                since,
                confirmed_at: at,
                resets_at: end,
                before_used_tenths: previous.filter(|_| same_period).map(|old| old.used_tenths),
                used_tenths: incoming.used_tenths,
                source: UsageResetSource::Recorded,
            };
            if valid(&reset, incoming, at) {
                accepted.push(reset);
            }
        }
        if !same_period
            && incoming.minutes == Some(WEEK_MINUTES)
            && let (Some(previous), Some(end)) = (previous, incoming.resets_at)
            && previous.resets_at.is_some_and(|old| end > old)
        {
            let since = end - SignedDuration::from_secs(super::WEEK_SECONDS);
            if since <= at {
                accepted.push(UsageReset {
                    window: incoming.name.clone(),
                    since,
                    confirmed_at: at,
                    resets_at: end,
                    before_used_tenths: Some(previous.used_tenths),
                    used_tenths: incoming.used_tenths,
                    source: UsageResetSource::Scheduled,
                });
            }
        }
        for reset in accepted {
            if !history.iter().any(|old| {
                old.window == reset.window
                    && old.since == reset.since
                    && super::readings::same(old.resets_at, reset.resets_at)
            }) {
                history.push(reset);
            }
        }
        let boundary = history
            .iter()
            .filter(|reset| reset.source != UsageResetSource::Scheduled && valid(reset, incoming, at))
            .max_by_key(|reset| reset.since);
        let since = boundary.map(|reset| reset.since);
        let mut merged = LimitWindow { since, ..incoming.clone() };
        if let Some(index) = old {
            let previous = &known[index];
            if same_period {
                merged.resets_at = previous.resets_at;
                if since <= previous.since {
                    merged.used_tenths = previous.used_tenths.max(incoming.used_tenths);
                }
            }
            known[index] = merged;
        } else {
            known.push(merged);
        }
    }
    known.sort_by(|a, b| a.name.cmp(&b.name).then(a.minutes.cmp(&b.minutes)));
}
