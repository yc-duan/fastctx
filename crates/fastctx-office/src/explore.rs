//! Bounded document beginnings and evenly sampled location catalogues.

use crate::{Content, output::Output};
use std::collections::BTreeSet;

pub(crate) const WHOLE: usize = 10_000 * 4;
/// The start of a document whose outline says where the rest is.
pub(crate) const BRIEF: usize = 1_000 * 4;
/// The start of a document with no outline to give, and of a sheet or a deck.
pub(crate) const HEAD: usize = 2_000 * 4;
pub(crate) const ENTRIES: usize = 40;

#[derive(Default)]
pub(crate) struct Rows(BTreeSet<u32>);

impl Rows {
    pub fn include(&mut self, row: u32) {
        if self.0.len() < ENTRIES || self.0.last().is_some_and(|last| row < *last) {
            self.0.insert(row);
            if self.0.len() > ENTRIES {
                self.0.pop_last();
            }
        }
    }

    pub fn last(&self) -> Option<u32> {
        self.0.last().copied()
    }

    pub fn intersects(&self, range: &crate::spreadsheet::CellRange) -> bool {
        self.0.range(range.first.row..=range.last.row).next().is_some()
    }
}

pub(crate) struct Catalogue {
    total: usize,
    stride: usize,
    items: Vec<(usize, String)>,
    last: Option<String>,
    all: Option<Vec<String>>,
    all_cost: usize,
    limit: usize,
}

impl Catalogue {
    pub fn new() -> Self {
        Self { total: 0, stride: 1, items: Vec::new(), last: None, all: None, all_cost: 0, limit: WHOLE }
    }

    pub fn all(budget: usize) -> Self {
        Self { all: Some(Vec::new()), limit: budget, ..Self::new() }
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn push(&mut self, text: String) {
        let index = self.total;
        self.total += 1;
        let text = if text.len() <= self.limit { text } else { String::new() };
        if let Some(all) = &mut self.all {
            self.all_cost += text.len();
            if self.all_cost <= self.limit {
                all.push(text.clone());
            } else {
                self.all = None;
            }
        }
        self.last = Some(text.clone());
        if index.is_multiple_of(self.stride) {
            self.items.push((index, text));
        }
        if self.items.len() > ENTRIES * 2 {
            self.stride *= 2;
            self.items.retain(|(index, _)| index.is_multiple_of(self.stride));
        }
    }

    pub fn append(self, content: &mut Content, label: &str, budget: usize) {
        let mut items = self.items;
        if let Some(last) = self.last
            && items.last().is_none_or(|(index, _)| *index != self.total - 1)
        {
            items.push((self.total - 1, last));
        }
        let all_header = format!("=== {label} ({n} of {n} entries shown) ===\n", n = self.total);
        let header = header_reserve(label);
        let mut left = budget.saturating_sub(content.facts.len() + content.text.len() + all_header.len());
        let all = self.all.filter(|_| self.all_cost <= left);
        let show_all = all.is_some();
        if let Some(all) = all {
            items = all.into_iter().enumerate().collect();
        }
        if !show_all {
            left = budget.saturating_sub(content.facts.len() + content.text.len() + header);
        }
        items.retain(|(_, item)| !item.is_empty() && item.len() <= left);
        let at = |i: usize, count: usize| if count <= 1 { 0 } else { i * (items.len() - 1) / (count - 1) };
        let mut count = if show_all { items.len() } else { items.len().min(ENTRIES) };
        while count > 0 && (0..count).map(|i| items[at(i, count)].1.len()).sum::<usize>() > left {
            count -= 1;
        }
        let mut text = String::new();
        for i in 0..count {
            text.push_str(&items[at(i, count)].1);
        }
        append_note(content, &format!("=== {label} ({count} of {} entries shown) ===\n{text}", self.total), budget);
    }
}

pub(crate) fn header_reserve(label: &str) -> usize {
    format!("=== {label} ({n} of {n} entries shown) ===\n", n = usize::MAX).len()
}

pub(crate) fn head_budget(budget: usize, label: &str, notes: usize) -> usize {
    head_within(budget, label, notes, HEAD)
}

pub(crate) fn head_within(budget: usize, label: &str, notes: usize, head: usize) -> usize {
    budget.saturating_sub(header_reserve(label) + notes).min(head)
}

pub(crate) fn append_note(content: &mut Content, note: &str, budget: usize) {
    if content.facts.len() + content.text.len() + note.len() <= budget {
        content.text.push_str(note);
    } else {
        content.text.clear();
        content.stopped = true;
    }
}

pub(crate) fn numbered(text: &str, budget: usize, probe: bool) -> Content {
    let mut out = if probe { Output::probe(budget) } else { Output::new(budget) };
    out.reserve_facts(crate::output::line_reserve().len());
    let lines = out.numbered(text, None, |line, total| format!("the rest is lines {line}-{total}"));
    out.facts(lines.facts(false));
    out.finish()
}
