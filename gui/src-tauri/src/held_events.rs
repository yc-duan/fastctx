//! The events a conversation's transcript holds, kept small: the latest as
//! they came, the older folded into compressed runs. A page that reads far
//! back unfolds only the runs it reads, the last one kept unfolded for the
//! read after it.

use std::cell::RefCell;
use std::io::{Read as _, Write as _};

use serde_json::value::RawValue;

/// Events folded together.
const RUN: usize = 256;
/// The latest events always kept as they came: well over a page's part.
const OPEN: usize = 1024;

#[derive(Default)]
pub(crate) struct HeldEvents {
    /// Folded runs of `RUN` events each, oldest first.
    runs: Vec<Vec<u8>>,
    /// The events after the folded runs, as they came.
    open: Vec<Box<RawValue>>,
    /// The run last unfolded, by its place among the runs.
    unfolded: RefCell<Option<(usize, Vec<Box<RawValue>>)>>,
}

impl HeldEvents {
    pub(crate) fn len(&self) -> usize {
        self.runs.len() * RUN + self.open.len()
    }

    pub(crate) fn push(&mut self, event: Box<RawValue>) -> Result<(), String> {
        self.open.push(event);
        if self.open.len() >= OPEN + RUN {
            let folded = fold(&self.open[..RUN])?;
            self.open.drain(..RUN);
            self.runs.push(folded);
        }
        Ok(())
    }

    pub(crate) fn get(&self, at: usize) -> Box<RawValue> {
        self.range(at, at + 1).pop().expect("an event held")
    }

    /// The events from `start` to `end`, as they came.
    pub(crate) fn range(&self, start: usize, end: usize) -> Vec<Box<RawValue>> {
        let folded = self.runs.len() * RUN;
        let mut events = Vec::with_capacity(end.saturating_sub(start));
        let mut at = start;
        while at < end.min(folded) {
            let run = at / RUN;
            let mut unfolded = self.unfolded.borrow_mut();
            if unfolded.as_ref().is_none_or(|(which, _)| *which != run) {
                *unfolded = Some((run, unfold(&self.runs[run])));
            }
            let (_, events_of_run) = unfolded.as_ref().expect("unfolded above");
            let until = end.min((run + 1) * RUN);
            events.extend(events_of_run[at - run * RUN..until - run * RUN].iter().cloned());
            at = until;
        }
        if at < end {
            events.extend(self.open[at - folded..end - folded].iter().cloned());
        }
        events
    }
}

/// Events as one compressed run: each its length and its text.
fn fold(events: &[Box<RawValue>]) -> Result<Vec<u8>, String> {
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    for event in events {
        let text = event.get().as_bytes();
        let length = u32::try_from(text.len()).map_err(|_| "an event too large to hold".to_string())?;
        encoder.write_all(&length.to_le_bytes()).map_err(|error| error.to_string())?;
        encoder.write_all(text).map_err(|error| error.to_string())?;
    }
    encoder.finish().map_err(|error| error.to_string())
}

/// A run's events back as they came; a run is only ever what `fold` made.
fn unfold(run: &[u8]) -> Vec<Box<RawValue>> {
    let mut bytes = Vec::new();
    flate2::read::DeflateDecoder::new(run).read_to_end(&mut bytes).expect("a folded run reads back");
    let mut events = Vec::with_capacity(RUN);
    let mut at = 0;
    while at < bytes.len() {
        let length = u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes")) as usize;
        at += 4;
        let text = String::from_utf8(bytes[at..at + length].to_vec()).expect("a folded event is text");
        events.push(RawValue::from_string(text).expect("a folded event is JSON"));
        at += length;
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_back_what_it_holds_across_runs() {
        let mut held = HeldEvents::default();
        for index in 0..5000 {
            held.push(RawValue::from_string(format!("{{\"at\":{index},\"text\":\"event {index}\"}}")).unwrap())
                .unwrap();
        }
        assert_eq!(held.len(), 5000);
        assert!(!held.runs.is_empty());
        let text = |at: usize| format!("{{\"at\":{at},\"text\":\"event {at}\"}}");
        for (start, end) in [(0, 3), (250, 270), (255, 257), (1000, 4999), (4990, 5000), (0, 5000)] {
            let read = held.range(start, end);
            assert_eq!(read.len(), end - start);
            for (offset, event) in read.iter().enumerate() {
                assert_eq!(event.get(), text(start + offset));
            }
        }
        assert_eq!(held.get(1234).get(), text(1234));
    }
}
