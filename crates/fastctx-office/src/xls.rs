use crate::{
    Selection,
    output::{Output, quoted},
    spreadsheet::{self, CellRange, Metrics},
};
use anyhow::{Result, bail};
use office_oxide::xls::CellValue;
use office_oxide::xlsx::CellRef;
use std::collections::BTreeSet;
use std::fs::File;

pub(crate) fn read(
    document: &office_oxide::Document,
    file: File,
    selection: &Selection,
    out: &mut Output,
) -> Result<()> {
    let workbook = document.as_xls().unwrap();
    if workbook.truncated() {
        bail!("XLS content truncated: {}", workbook.notices().join("; "));
    }
    let selected =
        spreadsheet::selected_sheet(&workbook.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), selection)?;
    let source = crate::biff::Workbook::read(file, workbook.sheets.iter().map(|s| s.name.clone()).collect())?;
    out.reserve_facts(spreadsheet::workbook_reserve());
    let (mut first, mut last) = (0, 0);
    for (index, sheet) in workbook.sheets.iter().enumerate() {
        if selected.is_some_and(|s| index != s) {
            continue;
        }
        let formulas = source.sheet(index)?;
        let mut addresses: BTreeSet<(u16, u16)> = formulas.positions().collect();
        for (row, cells) in sheet.rows.iter().enumerate() {
            for (col, value) in cells.iter().enumerate() {
                if !matches!(value, CellValue::Empty) {
                    addresses.insert((row as u16, col as u16));
                }
            }
        }
        let mut metrics = Metrics::default();
        for &(row, col) in &addresses {
            metrics.include(&CellRef { row: row.into(), col: col.into() });
        }
        for &(r1, r2, c1, c2) in &sheet.merged_cells {
            metrics.extend(&CellRef { row: r1.into(), col: c1.into() });
            metrics.extend(&CellRef { row: r2.into(), col: c2.into() });
        }
        let Some(header) = out.header(
            &format!("sheet {} {}", index + 1, quoted(&sheet.name)),
            &spreadsheet::sheet_reserve(),
            &format!("the rest starts at sheet {}", index + 1),
        ) else {
            break;
        };
        if first == 0 {
            first = index + 1;
        }
        last = index + 1;
        for &(r1, r2, c1, c2) in &sheet.merged_cells {
            let merged = CellRange {
                first: CellRef { row: r1.into(), col: c1.into() },
                last: CellRef { row: r2.into(), col: c2.into() },
            };
            if selection.range.as_ref().is_none_or(|r| r.intersects(&merged))
                && !out.line(
                    "",
                    &format!("merged {}:{}", merged.first, merged.last),
                    &format!("the rest starts at sheet {}, merged {}:{}", index + 1, merged.first, merged.last),
                )
            {
                break;
            }
        }
        for range in formulas.array_ranges() {
            if selection.range.as_ref().is_none_or(|r| r.intersects(&range))
                && !out.line(
                    "",
                    &format!("array {}:{}", range.first, range.last),
                    &format!("the rest starts at sheet {}, array {}:{}", index + 1, range.first, range.last),
                )
            {
                break;
            }
        }
        let (mut shown, mut through) = (0, None);
        for (row, col) in addresses {
            let address = CellRef { row: row.into(), col: col.into() };
            if selection.range.as_ref().is_some_and(|r| !r.contains(&address)) {
                continue;
            }
            let value = sheet.rows.get(row as usize).and_then(|r| r.get(col as usize)).unwrap_or(&CellValue::Empty);
            let xf = sheet.xf.get(row as usize).and_then(|r| r.get(col as usize)).copied().unwrap_or(0);
            let value = format_cell(value, xf, &formulas, &source, (row, col))?;
            if !out.cell(
                address.row,
                &address.to_string(),
                &value,
                &spreadsheet::resume(index, &address, selection.range.as_ref()),
            ) {
                break;
            }
            shown += 1;
            through = Some(address);
        }
        let mut facts =
            metrics.facts(selection.range.as_ref(), shown, if out.stopped() { through.as_ref() } else { None });
        if sheet.hidden {
            facts.push_str("; hidden");
        }
        out.update_header(header, &facts);
        if out.stopped() {
            break;
        }
    }
    out.facts(spreadsheet::workbook_facts(workbook.sheets.len(), first, last, selected.is_some() || out.stopped()));
    Ok(())
}

pub(crate) fn format_cell(
    value: &CellValue,
    xf: u16,
    formulas: &crate::biff::Sheet<'_>,
    source: &crate::biff::Workbook,
    (row, col): (u16, u16),
) -> Result<String> {
    let cache = match formulas.string_cache((row, col)) {
        Some(Ok(text)) => quoted(text),
        Some(Err(reason)) => format!("(cached value unavailable: {})", quoted(reason)),
        None => match value {
            CellValue::String(s) => quoted(s),
            CellValue::Empty => "(no cached value)".into(),
            CellValue::Bool(v) => v.to_string(),
            CellValue::Error(_) => format!("error {}", value.as_text()),
            CellValue::Number(n) => {
                let (id, format) = source.number_format(xf)?;
                crate::number::saved(&n.to_string(), *n, format, Some(id), source.date1904)
            }
        },
    };
    Ok(match formulas.formula((row, col)) {
        Some(Ok(formula)) => format!("{} → {cache}", crate::output::formula(&formula)),
        Some(Err(error)) => format!("(formula unavailable: {}) → {cache}", quoted(&format!("{error:#}"))),
        None => cache,
    })
}
