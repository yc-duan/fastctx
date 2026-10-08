use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use calamine::Reader;
use office_oxide::xlsx::CellRef;

use crate::{
    Selection,
    output::{Output, quoted},
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

pub(crate) fn resume(sheet: usize, cell: &CellRef, range: Option<&CellRange>) -> String {
    let last = range.map(|r| r.last.clone()).unwrap_or(CellRef { row: 1_048_575, col: 16_383 });
    let row_end = CellRef { row: cell.row, col: last.col };
    let mut hint = format!("use --sheet-index {} --range {cell}:{row_end}", sheet + 1);
    if cell.row < last.row {
        let next = CellRef { row: cell.row + 1, col: range.map_or(0, |r| r.first.col) };
        hint.push_str(&format!("; then --range {next}:{last} on that sheet"));
    }
    hint.push_str("; later sheets: --sheet-index NUMBER");
    hint
}

pub(crate) fn read_xls(
    document: &office_oxide::Document,
    mut file: File,
    selection: &Selection,
    out: &mut Output,
) -> Result<()> {
    use office_oxide::xls::CellValue;
    let workbook = document.as_xls().unwrap();
    if workbook.truncated() {
        bail!("XLS content truncated: {}", workbook.notices().join("; "));
    }
    let selected = selected_sheet(&workbook.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), selection)?;
    // office_oxide 0.1.13 reads the FORMULA record's cached value but
    // discards its tokens. Calamine decodes that external BIFF contract.
    let formula_cells = crate::biff::formula_cells(file.try_clone()?)?;
    if formula_cells.len() != workbook.sheets.len() {
        bail!("XLS sheet count differs between the directory and the parsed workbook");
    }
    let mut needs_formulas = false;
    for (index, cells) in formula_cells.iter().enumerate() {
        if selected.is_some_and(|wanted| index != wanted) {
            continue;
        }
        for cell in cells {
            let address = CellRef { row: cell.row as u32, col: cell.col as u32 };
            if selection.range.as_ref().is_some_and(|range| !range.contains(&address)) {
                continue;
            }
            if cell.shared_or_array {
                bail!(
                    "sheet {} {address}: XLS shared/array formulas are unsupported",
                    quoted(&workbook.sheets[index].name)
                );
            }
            needs_formulas = true;
        }
    }
    file.seek(SeekFrom::Start(0))?;
    let mut formulas =
        if needs_formulas { Some(calamine::Xls::new(file).context("reading XLS formula records")?) } else { None };
    if !out.line(
        "",
        &format!("=== Excel ({} sheets; formula values are saved caches) ===", workbook.sheets.len()),
        "use --sheet-index 1",
    ) {
        return Ok(());
    }
    for (index, sheet) in workbook.sheets.iter().enumerate() {
        if selected.is_some_and(|wanted| index != wanted) {
            continue;
        }
        let state = if sheet.hidden { "; hidden" } else { "" };
        if !out.line(
            "",
            &format!("=== sheet {} {}{state} ===", index + 1, quoted(&sheet.name)),
            &format!("use --sheet-index {}", index + 1),
        ) {
            break;
        }
        for &(r1, r2, c1, c2) in &sheet.merged_cells {
            let first = CellRef { row: r1.into(), col: c1.into() };
            let last = CellRef { row: r2.into(), col: c2.into() };
            if selection
                .range
                .as_ref()
                .is_none_or(|r| r.intersects(&CellRange { first: first.clone(), last: last.clone() }))
                && !out.line(
                    "",
                    &format!("merge {first}:{last}"),
                    &format!("use --sheet-index {} --range {first}:{last}", index + 1),
                )
            {
                return Ok(());
            }
        }
        let sheet_formulas = match &mut formulas {
            Some(formulas) => formulas.worksheet_formula(&sheet.name).context("decoding XLS formulas")?,
            None => calamine::Range::empty(),
        };
        let mut addresses = BTreeSet::new();
        for cell in &formula_cells[index] {
            let (row, col) = (cell.row, cell.col);
            let address = CellRef { row: row as u32, col: col as u32 };
            if selection.range.as_ref().is_some_and(|range| !range.contains(&address)) {
                continue;
            }
            match sheet_formulas.get_value((row as u32, col as u32)) {
                Some(formula) if !formula.is_empty() && !formula.starts_with("Unrecognised formula ") => {}
                _ => bail!(
                    "sheet {} {address}: XLS formula expression could not be decoded (shared/array formulas are unsupported)",
                    quoted(&sheet.name)
                ),
            }
            addresses.insert((row, col));
        }
        for (row, cells) in sheet.rows.iter().enumerate() {
            for (col, value) in cells.iter().enumerate() {
                if !matches!(value, CellValue::Empty) {
                    addresses.insert((row, col));
                }
            }
        }
        if let Some((r, c)) = sheet_formulas.start() {
            for (row, col, _) in sheet_formulas.used_cells() {
                addresses.insert((row + r as usize, col + c as usize));
            }
        }
        for (row, col) in addresses {
            let value = sheet.rows.get(row).and_then(|cells| cells.get(col)).unwrap_or(&CellValue::Empty);
            let address = CellRef { row: row as u32, col: col as u32 };
            if selection.range.as_ref().is_some_and(|r| !r.contains(&address)) {
                continue;
            }
            let formula = sheet_formulas.get_value((row as u32, col as u32)).filter(|s| !s.is_empty());
            if matches!(value, CellValue::Empty) && formula.is_none() {
                continue;
            }
            let mut text = match value {
                CellValue::String(s) => quoted(s),
                CellValue::Empty => "(no cached value)".to_owned(),
                CellValue::Bool(value) => value.to_string(),
                CellValue::Error(_) => format!("error {}", value.as_text()),
                _ => value.as_text(),
            };
            if let Some(formula) = formula {
                text = format!("={formula}\t=> {text}");
            }
            if !out.line(&format!("{address}\t"), &text, &resume(index, &address, selection.range.as_ref())) {
                return Ok(());
            }
        }
    }
    Ok(())
}
