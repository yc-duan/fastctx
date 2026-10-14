//! XLS previews and worksheet extents collected from one BIFF metadata scan.

use crate::{
    biff::Workbook,
    explore::{Catalogue, Rows, append_note, head_budget},
    output::{Output, count, quoted},
    spreadsheet::{self, CellRange, Metrics},
};
use anyhow::{Result, bail};
use office_oxide::{xls::CellValue, xlsx::CellRef};
use std::{collections::BTreeSet, fs::File};

pub(crate) fn explore(
    document: &office_oxide::Document,
    file: File,
    budget: usize,
    whole_budget: usize,
) -> Result<crate::Content> {
    let workbook = document.as_xls().unwrap();
    if workbook.truncated() {
        bail!("XLS content truncated: {}", workbook.notices().join("; "));
    }
    let source = Workbook::read(file, workbook.sheets.iter().map(|s| s.name.clone()).collect())?;
    let mut whole = if budget >= whole_budget { Output::probe(whole_budget) } else { Output::new(budget) };
    whole.reserve_facts(spreadsheet::workbook_reserve());
    let notes = format!(
        "=== sheet 1 ({} not shown) ===\n=== sheets 2-{} (0 of {} shown) ===\n",
        count(u64::MAX, "cell"),
        workbook.sheets.len(),
        count(u64::MAX, "cell")
    );
    let head_limit = head_budget(budget, "sheets", notes.len());
    let mut head = Output::new(head_limit);
    head.reserve_facts(spreadsheet::workbook_reserve());
    let mut catalogue = Catalogue::all(budget);
    let (mut first_cells, mut other_cells) = (0, 0);
    let (mut first, mut last) = (0, 0);
    for (index, sheet) in workbook.sheets.iter().enumerate() {
        let formulas = source.sheet(index)?;
        let mut addresses: BTreeSet<_> = formulas.positions().collect();
        for (row, cells) in sheet.rows.iter().enumerate() {
            for (col, value) in cells.iter().enumerate() {
                if !matches!(value, CellValue::Empty) {
                    addresses.insert((row as u16, col as u16));
                }
            }
        }
        let mut metrics = Metrics::default();
        let mut rows = Rows::default();
        for &(row, col) in &addresses {
            metrics.include(&CellRef { row: row.into(), col: col.into() });
            if index == 0 {
                rows.include(row.into());
            }
        }
        let merges: Vec<_> = sheet
            .merged_cells
            .iter()
            .map(|&(r1, r2, c1, c2)| CellRange {
                first: CellRef { row: r1.into(), col: c1.into() },
                last: CellRef { row: r2.into(), col: c2.into() },
            })
            .collect();
        for range in &merges {
            metrics.extend(&range.first);
            metrics.extend(&range.last);
        }
        if index == 0 {
            first_cells = metrics.cells;
        } else {
            other_cells += metrics.cells;
        }
        let extent = metrics.bounds.as_ref().map_or("empty, ".into(), |r| format!("{}:{}, ", r.first, r.last));
        let state = if sheet.hidden { "; hidden" } else { "" };
        catalogue.push(format!(
            "=== sheet {} {} ({extent}{}{state}) ===\n",
            index + 1,
            quoted(&sheet.name),
            count(metrics.cells, "cell")
        ));
        if whole.stopped() && index > 0 {
            continue;
        }
        let label = format!("sheet {} {}", index + 1, quoted(&sheet.name));
        let resume = format!("the rest starts at sheet {}", index + 1);
        let whole_header = whole.header(&label, &spreadsheet::sheet_reserve(), &resume);
        let head_header = (index == 0).then(|| head.header(&label, &spreadsheet::sheet_reserve(), &resume)).flatten();
        if whole_header.is_some() {
            if first == 0 {
                first = index + 1;
            }
            last = index + 1;
        }
        for (kind, ranges) in [("merged", merges), ("array", formulas.array_ranges())] {
            for range in ranges {
                let text = format!("{kind} {}:{}", range.first, range.last);
                let resume = format!("the rest starts at sheet {}, {text}", index + 1);
                if whole_header.is_some() {
                    whole.line("", &text, &resume);
                }
                if head_header.is_some() && rows.intersects(&range) {
                    head.line("", &text, &resume);
                }
            }
        }
        let (mut shown, mut through) = (0, None);
        let (mut head_shown, mut head_through) = (0, None);
        for (row, col) in addresses {
            let head_wanted = index == 0 && rows.last().is_some_and(|last| u32::from(row) <= last) && !head.stopped();
            if whole.stopped() && !head_wanted {
                break;
            }
            let address = CellRef { row: row.into(), col: col.into() };
            let value = sheet.rows.get(row as usize).and_then(|r| r.get(col as usize)).unwrap_or(&CellValue::Empty);
            let style = sheet.xf.get(row as usize).and_then(|r| r.get(col as usize)).copied().unwrap_or(0);
            let value = crate::xls::format_cell(value, style, &formulas, &source, (row, col))?;
            let resume = spreadsheet::resume(index, &address, None);
            if !whole.stopped() && whole.cell(address.row, &address.to_string(), &value, &resume) {
                shown += 1;
                through = Some(address.clone());
            }
            if head_wanted && head.cell(address.row, &address.to_string(), &value, &resume) {
                head_shown += 1;
                head_through = Some(address);
            }
        }
        if let Some(header) = whole_header {
            let mut facts = metrics.facts(None, shown, if whole.stopped() { through.as_ref() } else { None });
            facts.push_str(state);
            whole.update_header(header, &facts);
        }
        if let Some(header) = head_header {
            let mut facts = metrics.facts(None, head_shown, head_through.as_ref());
            facts.push_str(state);
            head.update_header(header, &facts);
        }
    }
    whole.facts(spreadsheet::workbook_facts(workbook.sheets.len(), first, last, whole.stopped()));
    let mut whole = whole.finish();
    if !whole.stopped && whole.facts.len() + whole.text.len() <= whole_budget {
        whole.whole = !whole.stopped;
        return Ok(whole);
    }
    head.facts(spreadsheet::workbook_facts(workbook.sheets.len(), 1, 1, true));
    let unseen = first_cells.saturating_sub(head.cell_count());
    let mut head = head.finish();
    if unseen > 0 {
        append_note(&mut head, &format!("=== sheet 1 ({} not shown) ===\n", count(unseen, "cell")), budget);
    }
    if other_cells > 0 {
        append_note(
            &mut head,
            &format!("=== sheets 2-{} (0 of {} shown) ===\n", workbook.sheets.len(), count(other_cells, "cell")),
            budget,
        );
    }
    catalogue.append(&mut head, "sheets", budget);
    head.stopped = (head_limit < crate::explore::HEAD && head.stopped) || head.text.is_empty();
    Ok(head)
}
