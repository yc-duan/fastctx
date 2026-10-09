use std::str::FromStr;

use anyhow::{Context, Result, bail};
use office_oxide::xlsx::CellRef;

use crate::{
    Selection,
    output::{count, quoted},
};

/// An inclusive A1 rectangle within Excel's grid. Single cells are valid.
#[derive(Debug, Clone)]
pub struct CellRange {
    pub first: CellRef,
    pub last: CellRef,
}

impl FromStr for CellRange {
    type Err = anyhow::Error;

    fn from_str(text: &str) -> Result<Self> {
        let (first, last) = text.split_once(':').unwrap_or((text, text));
        let first = CellRef::parse(first).context("expected an A1 cell or rectangle, such as A1:D20")?;
        let last = CellRef::parse(last).context("expected an A1 cell or rectangle, such as A1:D20")?;
        if first.row > last.row || first.col > last.col {
            bail!("range ends before it starts");
        }
        Ok(Self { first, last })
    }
}

impl CellRange {
    pub(crate) fn contains(&self, cell: &CellRef) -> bool {
        (self.first.row..=self.last.row).contains(&cell.row) && (self.first.col..=self.last.col).contains(&cell.col)
    }

    pub(crate) fn intersects(&self, other: &Self) -> bool {
        self.first.row <= other.last.row
            && self.last.row >= other.first.row
            && self.first.col <= other.last.col
            && self.last.col >= other.first.col
    }
}

pub(crate) fn selected_sheet(names: &[&str], selection: &Selection) -> Result<Option<usize>> {
    if selection.sheet.is_some() && selection.sheet_index.is_some() {
        bail!("select a sheet by name or index, not both");
    }
    if let Some(name) = &selection.sheet {
        return names
            .iter()
            .position(|n| *n == name)
            .map(Some)
            .with_context(|| format!("no sheet {}; workbook has {} sheets", quoted(name), names.len()));
    }
    if let Some(number) = selection.sheet_index {
        if number == 0 || number > names.len() {
            bail!("no sheet index {number}; workbook has {} sheets", names.len());
        }
        return Ok(Some(number - 1));
    }
    Ok(None)
}

/// Where the cells not shown start: a fact, not a command, so the reader
/// picks its own way to read on.
pub(crate) fn resume(sheet: usize, cell: &CellRef, _range: Option<&CellRange>) -> String {
    format!("the rest starts at {cell} on sheet {}", sheet + 1)
}

#[derive(Default)]
pub(crate) struct Metrics {
    pub cells: u64,
    pub bounds: Option<CellRange>,
}
impl Metrics {
    pub fn include(&mut self, cell: &CellRef) {
        self.cells += 1;
        self.extend(cell);
    }
    pub fn extend(&mut self, cell: &CellRef) {
        match &mut self.bounds {
            None => self.bounds = Some(CellRange { first: cell.clone(), last: cell.clone() }),
            Some(r) => {
                r.first.row = r.first.row.min(cell.row);
                r.first.col = r.first.col.min(cell.col);
                r.last.row = r.last.row.max(cell.row);
                r.last.col = r.last.col.max(cell.col);
            }
        }
    }
    pub fn facts(&self, range: Option<&CellRange>, shown: u64, through: Option<&CellRef>) -> String {
        let extent = self.bounds.as_ref().map(|r| format!("{}:{}", r.first, r.last));
        let area = match (range, extent) {
            (Some(r), Some(all)) => format!("{}:{} of {all}, ", r.first, r.last),
            (Some(r), None) => format!("{}:{}, ", r.first, r.last),
            (None, Some(all)) => format!("{all}, "),
            _ => String::new(),
        };
        let cells = if range.is_none() && shown == self.cells {
            count(shown, "cell")
        } else {
            format!("{shown} of {}", count(self.cells, "cell"))
        };
        match through {
            Some(cell) => format!("{area}{cells}; through {cell}"),
            None => format!("{area}{cells}"),
        }
    }
}
pub(crate) fn sheet_reserve() -> String {
    format!(
        "XFD1048576:XFD1048576 of XFD1048576:XFD1048576, {n} of {n} cells; through XFD1048576; very hidden",
        n = u64::MAX
    )
}
pub(crate) fn workbook_reserve() -> usize {
    format!("sheets {n}-{n} of {n}; formula values as last saved", n = u64::MAX).len()
}
pub(crate) fn workbook_facts(total: usize, first: usize, last: usize, selected: bool) -> String {
    let sheets = if total == 0 {
        count(0, "sheet")
    } else if first == 0 {
        format!("no sheets shown of {total}")
    } else if !selected && first == 1 && last == total {
        count(total as u64, "sheet")
    } else if first == last {
        format!("sheet {first} of {total}")
    } else {
        format!("sheets {first}-{last} of {total}")
    };
    format!("{sheets}; formula values as last saved")
}
