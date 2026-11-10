//! Where the events of a history stand in the timeline: an order key each,
//! compared as bytes. Events that were there and still follow one another
//! keep their keys; a new one takes a key between its neighbours', or past
//! the last. Keys are spaced so that what comes between usually fits; where
//! it does not, the keys from there on are given anew.

/// The space left between two keys as they are first given.
pub(super) const GAP: u64 = 1 << 20;

/// The order key of each event of a history, given the key each had when it
/// was in the timeline as it is now.
pub(super) fn assign(had: &[Option<u64>]) -> Vec<u64> {
    let mut kept = rising(had);
    let mut keys = vec![0u64; had.len()];
    let mut left = 0u64;
    let mut at = 0;
    while at < had.len() {
        if kept[at] {
            left = had[at].expect("a kept event had a key");
            keys[at] = left;
            at += 1;
            continue;
        }
        let end = (at..had.len()).find(|index| kept[*index]).unwrap_or(had.len());
        let fresh = (end - at) as u64;
        match had.get(end).copied().flatten() {
            Some(right) if right - left > fresh => {
                let step = (right - left) / (fresh + 1);
                for key in &mut keys[at..end] {
                    left += step;
                    *key = left;
                }
            }
            Some(_) => {
                // No room before the next kept key: none after here is kept.
                kept[end..].fill(false);
                continue;
            }
            None => {
                for key in &mut keys[at..end] {
                    left = left.saturating_add(GAP);
                    *key = left;
                }
            }
        }
        at = end;
    }
    keys
}

/// Which events keep their keys: the most of them whose keys rise in the
/// history's order.
fn rising(had: &[Option<u64>]) -> Vec<bool> {
    // `tails[length]` is the event ending the rising run of that length with
    // the least key; `before` links each event to the one before it in its run.
    let mut tails: Vec<usize> = Vec::new();
    let mut before: Vec<Option<usize>> = vec![None; had.len()];
    for (index, key) in had.iter().enumerate() {
        let Some(key) = key else { continue };
        let length = tails.partition_point(|tail| had[*tail].expect("a tail has a key") < *key);
        before[index] = length.checked_sub(1).map(|shorter| tails[shorter]);
        match tails.get_mut(length) {
            Some(tail) => *tail = index,
            None => tails.push(index),
        }
    }
    let mut kept = vec![false; had.len()];
    let mut next = tails.last().copied();
    while let Some(index) = next {
        kept[index] = true;
        next = before[index];
    }
    kept
}
