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
    assign_with(had.len(), &|index| had[index])
}

/// `assign` of `count` events, the key each had asked of `had`: nothing held
/// for it of them but their new keys and a few bytes each while it decides.
pub(super) fn assign_with(count: usize, had: &dyn Fn(usize) -> Option<u64>) -> Vec<u64> {
    let mut kept = rising(count, had);
    let mut keys = vec![0u64; count];
    let mut left = 0u64;
    let mut at = 0;
    while at < count {
        if kept[at] {
            left = had(at).expect("a kept event had a key");
            keys[at] = left;
            at += 1;
            continue;
        }
        let end = (at..count).find(|index| kept[*index]).unwrap_or(count);
        let fresh = (end - at) as u64;
        match if end < count { had(end) } else { None } {
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

/// Keeps each key of `keys` that is not `fixed` within the fixed keys
/// around it: past the one before it, and no further than the one after
/// it. A fixed key stays as it is — an event left as the timeline has it
/// (`Span::Added`) — so what is placed between two with no key between them
/// shares one with a neighbour.
pub(super) fn within(keys: &mut [u64], fixed: &[Option<u64>]) {
    within_with(keys, &|index| fixed[index]);
}

/// `within`, which keys are fixed asked of `fixed`: what is between two
/// fixed keys is found by reading on to the next, nothing held for each.
pub(super) fn within_with(keys: &mut [u64], fixed: &dyn Fn(usize) -> Option<u64>) {
    let mut before = None;
    let mut at = 0;
    while at < keys.len() {
        if let Some(key) = fixed(at) {
            keys[at] = key;
            before = Some(key);
            at += 1;
            continue;
        }
        // The run of those not fixed, and the fixed key after it.
        let end = (at..keys.len()).find(|index| fixed(*index).is_some()).unwrap_or(keys.len());
        let next = if end < keys.len() { fixed(end) } else { None };
        for key in &mut keys[at..end] {
            if let Some(next) = next {
                *key = (*key).min(next);
            }
            if let Some(before) = before {
                *key = (*key).max(before);
            }
        }
        at = end;
    }
}

/// No event, in `rising`.
const NONE: u32 = u32::MAX;

/// Which events keep their keys: the most of them whose keys rise in the
/// history's order.
fn rising(count: usize, had: &dyn Fn(usize) -> Option<u64>) -> Vec<bool> {
    // `tails[length]` is the event ending the rising run of that length with
    // the least key; `before` links each event to the one before it in its run.
    let mut tails: Vec<u32> = Vec::new();
    let mut before: Vec<u32> = vec![NONE; count];
    for (index, link) in before.iter_mut().enumerate() {
        let Some(key) = had(index) else { continue };
        let length = tails.partition_point(|tail| had(*tail as usize).expect("a tail has a key") < key);
        *link = length.checked_sub(1).map_or(NONE, |shorter| tails[shorter]);
        let number = u32::try_from(index).ok().filter(|number| *number < NONE).expect("fewer events than numbers");
        match tails.get_mut(length) {
            Some(tail) => *tail = number,
            None => tails.push(number),
        }
    }
    let mut kept = vec![false; count];
    let mut next = tails.last().copied();
    while let Some(index) = next {
        kept[index as usize] = true;
        next = Some(before[index as usize]).filter(|before| *before != NONE);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `assign`, `within` and `rising` as they were before they were asked
    /// of accessors, word for word: what they must equal.
    fn assign_before(had: &[Option<u64>]) -> Vec<u64> {
        let mut kept = rising_before(had);
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

    fn within_before(keys: &mut [u64], fixed: &[Option<u64>]) {
        let mut after = vec![None; keys.len()];
        let mut next = None;
        for at in (0..keys.len()).rev() {
            match fixed[at] {
                Some(key) => next = Some(key),
                None => after[at] = next,
            }
        }
        let mut before = None;
        for at in 0..keys.len() {
            match fixed[at] {
                Some(key) => {
                    keys[at] = key;
                    before = Some(key);
                }
                None => {
                    if let Some(next) = after[at] {
                        keys[at] = keys[at].min(next);
                    }
                    if let Some(before) = before {
                        keys[at] = keys[at].max(before);
                    }
                }
            }
        }
    }

    fn rising_before(had: &[Option<u64>]) -> Vec<bool> {
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

    /// Keys given and kept within fixed ones, asked of accessors, are those
    /// given and kept as before: over many histories of keys had and not,
    /// alike and apart, near and far, some fixed.
    #[test]
    fn keys_asked_of_accessors_are_the_keys_they_were() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut pick = |n: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % n
        };
        for round in 0..20_000 {
            let count = pick(40) as usize;
            let spread = [4, 50, GAP * 3, u64::MAX / 2][pick(4) as usize];
            let had: Vec<Option<u64>> = (0..count).map(|_| (pick(4) != 0).then(|| pick(spread))).collect();
            let was = assign_before(&had);
            assert_eq!(assign(&had), was, "round {round}: {had:?}");
            let fixed: Vec<Option<u64>> = had.iter().map(|key| key.filter(|_| pick(3) == 0)).collect();
            let (mut now, mut then) = (was.clone(), was);
            within(&mut now, &fixed);
            within_before(&mut then, &fixed);
            assert_eq!(now, then, "round {round}: {had:?} {fixed:?}");
        }
    }

    /// Placed between two left as they are with no key between them, a new
    /// one does not go past the second.
    #[test]
    fn a_new_one_stays_within_those_left_as_they_are() {
        let mut keys = assign(&[Some(1), None, Some(2)]);
        assert_eq!(keys, vec![1, 1 + GAP, 1 + 2 * GAP], "no room: given anew, past the second");
        within(&mut keys, &[Some(1), None, Some(2)]);
        assert_eq!(keys, vec![1, 2, 2]);
        let mut keys = vec![10, 20, 30, 40];
        within(&mut keys, &[None, Some(25), None, None]);
        assert_eq!(keys, vec![10, 25, 30, 40]);
        let mut keys = vec![10, 5];
        within(&mut keys, &[Some(10), None]);
        assert_eq!(keys, vec![10, 10]);
    }
}
