//! Stages explicit in-place replacements without walking unchanged entries.

use super::{Changes, Listed, ListedNext, Put, body};
use serde::Serialize;
use std::{io, sync::Arc};

pub(crate) enum Staged<T> {
    Unchanged,
    Replaced(Vec<(usize, Arc<T>)>),
    Compared(ListedNext<T>),
}

impl<T: Serialize> Listed<T> {
    pub(crate) fn stage(
        &self,
        now: &[Arc<T>],
        changed: bool,
        positions: Option<&[usize]>,
        key: impl Fn(&T) -> String,
        changes: &mut Changes,
    ) -> io::Result<Staged<T>> {
        if !changed {
            return Ok(Staged::Unchanged);
        }
        let Some(positions) = positions.filter(|_| !self.whole && now.len() == self.written.len()) else {
            return self.changes(now, key, changes).map(Staged::Compared);
        };
        if positions
            .iter()
            .any(|&at| self.written.get(at).zip(now.get(at)).is_none_or(|(written, entry)| key(entry) != *written.key))
        {
            return self.changes(now, key, changes).map(Staged::Compared);
        }
        let mut replaced = Vec::with_capacity(positions.len());
        for &at in positions {
            let written = &self.written[at];
            if !Arc::ptr_eq(&written.entry, &now[at]) {
                changes.put.push(Put {
                    kind: self.kind,
                    key: written.key.clone(),
                    position: written.position,
                    body: body(&*now[at])?,
                });
                replaced.push((at, now[at].clone()));
            }
        }
        Ok(Staged::Replaced(replaced))
    }

    pub(crate) fn keep_stage(&mut self, next: Staged<T>) {
        match next {
            Staged::Unchanged => {}
            Staged::Replaced(entries) => {
                for (at, entry) in entries {
                    self.written[at].entry = entry;
                }
            }
            Staged::Compared(next) => self.kept(next),
        }
    }
}
