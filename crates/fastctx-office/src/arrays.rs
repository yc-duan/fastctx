//! Sparse array-formula definitions and lazy enumeration of their cells.
use crate::spreadsheet::CellRange;
use anyhow::{Result, ensure};
use office_oxide::xlsx::CellRef;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

#[derive(Default)]
pub(crate) struct Arrays {
    pub groups: Vec<(CellRange, String)>,
}
impl Arrays {
    pub fn lookup(&self) -> Lookup<'_> {
        let mut starts: Vec<_> = (0..self.groups.len()).collect();
        starts.sort_by_key(|&i| (self.groups[i].0.first.row, self.groups[i].0.first.col));
        Lookup { arrays: self, starts, next: 0, active: BTreeMap::new(), ends: BinaryHeap::new(), row: 0 }
    }
    pub fn cells(&self, selected: Option<&CellRange>) -> Cells<'_> {
        let mut bounds = Vec::new();
        let mut next = BinaryHeap::new();
        for (id, (range, _)) in self.groups.iter().enumerate() {
            let mut range = range.clone();
            if let Some(wanted) = selected {
                if !range.intersects(wanted) {
                    continue;
                }
                range.first.row = range.first.row.max(wanted.first.row);
                range.first.col = range.first.col.max(wanted.first.col);
                range.last.row = range.last.row.min(wanted.last.row);
                range.last.col = range.last.col.min(wanted.last.col);
            }
            next.push(Reverse((range.first.row, range.first.col, bounds.len())));
            bounds.push((range, id));
        }
        Cells { arrays: self, bounds, next }
    }
}

pub(crate) struct Lookup<'a> {
    arrays: &'a Arrays,
    starts: Vec<usize>,
    next: usize,
    active: BTreeMap<u32, usize>,
    ends: BinaryHeap<Reverse<(u32, u32)>>,
    row: u32,
}
impl Lookup<'_> {
    fn expire(&mut self, row: u32) {
        while self.ends.peek().is_some_and(|Reverse((end, _))| *end < row) {
            let Reverse((_, col)) = self.ends.pop().unwrap();
            self.active.remove(&col);
        }
    }
    pub fn get(&mut self, cell: &CellRef) -> Result<bool> {
        ensure!(cell.row >= self.row, "worksheet rows are not in ascending order");
        self.row = cell.row;
        while let Some(&id) = self.starts.get(self.next) {
            let range = &self.arrays.groups[id].0;
            if range.first.row > cell.row {
                break;
            }
            self.expire(range.first.row);
            let before = self.active.range(..=range.first.col).next_back();
            let after = self.active.range(range.first.col..).next();
            ensure!(
                before.is_none_or(|(_, &i)| self.arrays.groups[i].0.last.col < range.first.col)
                    && after.is_none_or(|(&col, _)| col > range.last.col),
                "overlapping array formula ranges"
            );
            self.active.insert(range.first.col, id);
            self.ends.push(Reverse((range.last.row, range.first.col)));
            self.next += 1;
        }
        self.expire(cell.row);
        Ok(self
            .active
            .range(..=cell.col)
            .next_back()
            .is_some_and(|(_, &i)| self.arrays.groups[i].0.last.col >= cell.col))
    }
}

pub(crate) struct Cells<'a> {
    arrays: &'a Arrays,
    bounds: Vec<(CellRange, usize)>,
    next: BinaryHeap<Reverse<(u32, u32, usize)>>,
}
impl Cells<'_> {
    pub fn peek(&self) -> Option<(u32, u32)> {
        self.next.peek().map(|Reverse((r, c, _))| (*r, *c))
    }
}
impl<'a> Iterator for Cells<'a> {
    type Item = (CellRef, &'a str);
    fn next(&mut self) -> Option<Self::Item> {
        let Reverse((row, col, id)) = self.next.pop()?;
        let (range, group) = &self.bounds[id];
        if col < range.last.col {
            self.next.push(Reverse((row, col + 1, id)));
        } else if row < range.last.row {
            self.next.push(Reverse((row + 1, range.first.col, id)));
        }
        Some((CellRef { row, col }, self.arrays.groups[*group].1.as_str()))
    }
}
