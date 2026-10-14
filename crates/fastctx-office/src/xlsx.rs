//! Stream worksheet cells; retain shared resources and formula masters,
//! never the rectangular grid or a Markdown rendering of the workbook.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

use anyhow::{Context, Result, bail};
use office_oxide::core::{relationships::rel_types, xml};
use office_oxide::xlsx::{CellRef, SharedStringTable, SheetState, StyleSheet, WorkbookInfo};
use quick_xml::{Reader, events::Event};
use zip::ZipArchive;

use crate::output::{Output, quoted};
use crate::package::{self, attr};
use crate::spreadsheet::{CellRange, resume, selected_sheet};
use crate::{Content, Selection};

#[derive(Default)]
struct Cell {
    address: String,
    kind: String,
    style: Option<u32>,
    value: String,
    cached: bool,
    formula: Option<String>,
    formula_kind: String,
    shared: Option<u32>,
    formula_range: Option<String>,
}

impl Cell {
    fn has_content(&self) -> bool {
        self.formula.is_some()
            || self.cached && (!self.value.is_empty() || matches!(self.kind.as_str(), "inlineStr" | "str"))
    }
}

#[derive(Default)]
struct SheetFacts {
    merges: Vec<String>,
    masters: HashMap<u32, (CellRef, String)>,
    metrics: crate::spreadsheet::Metrics,
    arrays: crate::arrays::Arrays,
}

pub(crate) fn read(file: File, selection: &Selection, budget: usize, exploration: Option<usize>) -> Result<Content> {
    let exploring = exploration.is_some();
    let whole_budget = exploration.unwrap_or(budget);
    let mut zip = package::open(file)?;
    let root = office_oxide::core::relationships::Relationships::parse(&package::bytes(&mut zip, "_rels/.rels")?)?;
    let workbook_path =
        package::target("", root.first_by_type(rel_types::OFFICE_DOCUMENT).context("no workbook relationship")?)?;
    let mut workbook = WorkbookInfo::parse(&package::bytes(&mut zip, &workbook_path)?)?;
    for sheet in &mut workbook.sheets {
        sheet.name = crate::xstring::decode(&sheet.name)?;
    }
    let rels = package::relationships(&mut zip, &workbook_path)?;
    let strings = match rels.first_by_type(rel_types::SHARED_STRINGS) {
        Some(rel) => {
            let bytes = package::bytes(&mut zip, &package::target(&workbook_path, rel)?)?;
            // Normalize literal XML newlines before entity expansion, so
            // an intentional &#13; remains distinct from a literal CR.
            let text = std::str::from_utf8(&bytes)?.replace("\r\n", "\n").replace('\r', "\n");
            SharedStringTable::parse(text.as_bytes())?
        }
        None => SharedStringTable::empty(),
    };
    let styles = match rels.first_by_type(rel_types::STYLES) {
        Some(rel) => Some(StyleSheet::parse(&package::bytes(&mut zip, &package::target(&workbook_path, rel)?)?)?),
        None => None,
    };
    let selected = selected_sheet(&workbook.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), selection)?;
    let mut out = if exploring && budget >= whole_budget { Output::probe(whole_budget) } else { Output::new(budget) };
    let notes = format!(
        "=== sheet 1 ({} not shown) ===\n=== sheets 2-{} (0 of {} shown) ===\n",
        crate::output::count(u64::MAX, "cell"),
        workbook.sheets.len(),
        crate::output::count(u64::MAX, "cell")
    );
    let head_budget = crate::explore::head_budget(budget, "sheets", notes.len());
    let mut head = Output::new(head_budget);
    head.reserve_facts(crate::spreadsheet::workbook_reserve());
    let mut catalogue = crate::explore::Catalogue::all(budget);
    let (mut first_cells, mut other_cells) = (0, 0);
    out.reserve_facts(crate::spreadsheet::workbook_reserve());
    let (mut first, mut last) = (0, 0);
    for (index, sheet) in workbook.sheets.iter().enumerate() {
        if selected.is_some_and(|wanted| index != wanted) {
            continue;
        }
        let rel = rels.get_by_id(&sheet.rel_id).context("missing worksheet relationship")?;
        if !matches!(rel.rel_type.as_str(), rel_types::WORKSHEET | rel_types::MACROSHEET | rel_types::INTL_MACROSHEET) {
            if exploring {
                catalogue.push(format!(
                    "=== sheet {} {} ({} content not shown) ===\n",
                    index + 1,
                    quoted(&sheet.name),
                    rel.rel_type.rsplit('/').next().unwrap_or(&rel.rel_type)
                ));
            }
            if !out.line(
                "",
                &format!(
                    "=== sheet {} {} ({} content not shown) ===",
                    index + 1,
                    quoted(&sheet.name),
                    rel.rel_type.rsplit('/').next().unwrap_or(&rel.rel_type)
                ),
                &format!("the rest starts at sheet {}", index + 1),
            ) {
                if exploring {
                    continue;
                }
                break;
            }
            if first == 0 {
                first = index + 1;
            }
            last = index + 1;
            continue;
        }
        let part = package::target(&workbook_path, rel)?;
        let mut facts = SheetFacts::default();
        let mut preview_rows = crate::explore::Rows::default();
        let mut previous = None;
        let mut ordered = true;
        scan(
            &mut zip,
            &part,
            |cell| {
                let address = CellRef::parse(&cell.address).context("invalid cell address")?;
                let position = (address.row, address.col);
                ordered &= previous.is_none_or(|p| p < position);
                previous = Some(position);
                if cell.has_content() {
                    facts.metrics.include(&address);
                    if exploring && index == 0 {
                        preview_rows.include(address.row);
                    }
                }
                if cell.formula_kind == "shared" && cell.formula.as_ref().is_some_and(|f| !f.is_empty()) {
                    let id = cell.shared.context("shared formula has no index")?;
                    let address = CellRef::parse(&cell.address).context("invalid formula address")?;
                    if facts.masters.insert(id, (address, cell.formula.clone().unwrap())).is_some() {
                        bail!("duplicate shared formula index {id}");
                    }
                }
                if cell.formula_kind == "array" {
                    let range = cell.formula_range.as_deref().context("array formula has no range")?.parse()?;
                    facts.arrays.groups.push((range, cell.formula.unwrap_or_default()));
                }
                Ok(true)
            },
            |merge| {
                facts.merges.push(merge);
                Ok(())
            },
        )?;
        if !facts.arrays.groups.is_empty() {
            anyhow::ensure!(ordered, "array formula expansion requires cells in row and column order");
            // Array members exist even when the package omits their cell
            // records. Count their union without expanding a rectangular grid.
            facts.arrays.lookup().get(&CellRef { row: 1_048_575, col: 16_383 })?;
            for (range, _) in &facts.arrays.groups {
                if exploring && index == 0 {
                    for row in (range.first.row..=range.last.row).take(crate::explore::ENTRIES) {
                        preview_rows.include(row);
                    }
                }
                facts.metrics.cells +=
                    u64::from(range.last.row - range.first.row + 1) * u64::from(range.last.col - range.first.col + 1);
                facts.metrics.extend(&range.first);
                facts.metrics.extend(&range.last);
            }
            let mut lookup = facts.arrays.lookup();
            scan(
                &mut zip,
                &part,
                |cell| {
                    let address = CellRef::parse(&cell.address).context("invalid cell address")?;
                    if lookup.get(&address)? && cell.has_content() {
                        facts.metrics.cells -= 1;
                    }
                    Ok(true)
                },
                |_| Ok(()),
            )?;
        }
        for merge in &facts.merges {
            let range: CellRange = merge.parse()?;
            facts.metrics.extend(&range.first);
            facts.metrics.extend(&range.last);
        }
        if exploring {
            if index == 0 {
                first_cells = facts.metrics.cells;
            } else {
                other_cells += facts.metrics.cells;
            }
            let extent =
                facts.metrics.bounds.as_ref().map_or("empty, ".into(), |r| format!("{}:{}, ", r.first, r.last));
            let state = match sheet.state {
                SheetState::Visible => "",
                SheetState::Hidden => "; hidden",
                SheetState::VeryHidden => "; very hidden",
            };
            catalogue.push(format!(
                "=== sheet {} {} ({extent}{}{state}) ===\n",
                index + 1,
                quoted(&sheet.name),
                crate::output::count(facts.metrics.cells, "cell")
            ));
            if out.stopped() {
                continue;
            }
        }
        let Some(header) = out.header(
            &format!("sheet {} {}", index + 1, quoted(&sheet.name)),
            &crate::spreadsheet::sheet_reserve(),
            &format!("the rest starts at sheet {}", index + 1),
        ) else {
            if exploring {
                continue;
            }
            break;
        };
        let head_header = if exploring && index == 0 {
            head.header(
                &format!("sheet 1 {}", quoted(&sheet.name)),
                &crate::spreadsheet::sheet_reserve(),
                "the rest starts at sheet 1",
            )
        } else {
            None
        };
        if head_header.is_some() {
            for merge in &facts.merges {
                if preview_rows.intersects(&merge.parse()?) {
                    head.line("", &format!("merged {merge}"), &format!("the rest starts at sheet 1, merged {merge}"));
                }
            }
            for (range, _) in &facts.arrays.groups {
                if !preview_rows.intersects(range) {
                    continue;
                }
                head.line(
                    "",
                    &format!("array {}:{}", range.first, range.last),
                    &format!("the rest starts at sheet 1, array {}:{}", range.first, range.last),
                );
            }
        }
        if first == 0 {
            first = index + 1;
        }
        last = index + 1;
        for merge in &facts.merges {
            let merged: CellRange = merge.parse()?;
            if selection.range.as_ref().is_none_or(|range| range.intersects(&merged))
                && !out.line(
                    "",
                    &format!("merged {merge}"),
                    &format!("the rest starts at sheet {}, merged {merge}", index + 1),
                )
            {
                break;
            }
        }
        for (range, _) in &facts.arrays.groups {
            if selection.range.as_ref().is_none_or(|r| r.intersects(range))
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
        let (mut head_shown, mut head_through) = (0, None);
        let (mut head_rows, mut head_row) = (0, None);
        if !out.stopped() {
            let mut array_cells = facts.arrays.cells(selection.range.as_ref());
            let mut emit = |address: CellRef, value: &str| {
                if exploring && index == 0 && !head.stopped() {
                    if head_row != Some(address.row) {
                        head_rows += 1;
                        head_row = Some(address.row);
                    }
                    if head_rows <= crate::explore::ENTRIES
                        && head.cell(address.row, &address.to_string(), value, &resume(index, &address, None))
                    {
                        head_shown += 1;
                        head_through = Some(address.clone());
                    }
                }
                if !out.cell(
                    address.row,
                    &address.to_string(),
                    value,
                    &resume(index, &address, selection.range.as_ref()),
                ) {
                    return false;
                }
                shown += 1;
                through = Some(address);
                true
            };
            let completed = scan(
                &mut zip,
                &part,
                |cell| {
                    let address = CellRef::parse(&cell.address).context("invalid cell address")?;
                    if selection.range.as_ref().is_some_and(|range| !range.contains(&address)) {
                        return Ok(true);
                    }
                    let position = (address.row, address.col);
                    while array_cells.peek().is_some_and(|next| next < position) {
                        let (address, formula) = array_cells.next().unwrap();
                        if !emit(address, &formula_value(Ok(formula.to_owned()), "(no cached value)")) {
                            return Ok(false);
                        }
                    }
                    let array = (array_cells.peek() == Some(position)).then(|| array_cells.next().unwrap().1);
                    if !cell.has_content() && array.is_none() {
                        return Ok(true);
                    }
                    let cache = value(&cell, &strings, styles.as_ref(), workbook.date1904)?;
                    let value = if let Some(formula) = &cell.formula {
                        let decoded = (|| -> Result<String> {
                            match cell.formula_kind.as_str() {
                                "dataTable" => bail!("data-table formula parameters are not decoded"),
                                "" | "normal" | "shared" | "array" => {}
                                other => bail!("unsupported formula type {other}"),
                            }
                            if cell.formula_kind == "shared" && formula.is_empty() {
                                let id = cell.shared.context("shared formula has no index")?;
                                let (from, master) = facts.masters.get(&id).context("missing shared formula master")?;
                                crate::xlsx_formula::translate(master, from, &address)
                            } else {
                                Ok(formula.clone())
                            }
                        })();
                        formula_value(decoded, &cache)
                    } else if let Some(formula) = array {
                        formula_value(Ok(formula.to_owned()), &cache)
                    } else {
                        cache
                    };
                    Ok(emit(address, &value))
                },
                |_| Ok(()),
            )?;
            if completed {
                for (address, formula) in array_cells {
                    if !emit(address, &formula_value(Ok(formula.to_owned()), "(no cached value)")) {
                        break;
                    }
                }
            }
        }
        let mut metric =
            facts.metrics.facts(selection.range.as_ref(), shown, if out.stopped() { through.as_ref() } else { None });
        match sheet.state {
            SheetState::Visible => {}
            SheetState::Hidden => metric.push_str("; hidden"),
            SheetState::VeryHidden => metric.push_str("; very hidden"),
        }
        out.update_header(header, &metric);
        if let Some(header) = head_header {
            let facts = facts.metrics.facts(None, head_shown, head_through.as_ref());
            head.update_header(header, &facts);
        }
        if out.stopped() && !exploring {
            break;
        }
    }
    out.facts(crate::spreadsheet::workbook_facts(
        workbook.sheets.len(),
        first,
        last,
        selected.is_some() || out.stopped(),
    ));
    let mut whole = out.finish();
    if !exploring || !whole.stopped && whole.facts.len() + whole.text.len() <= whole_budget {
        whole.whole = !whole.stopped;
        return Ok(whole);
    }
    head.facts(crate::spreadsheet::workbook_facts(workbook.sheets.len(), 1, 1, true));
    let unseen = first_cells.saturating_sub(head.cell_count());
    let mut head = head.finish();
    if unseen > 0 {
        crate::explore::append_note(
            &mut head,
            &format!("=== sheet 1 ({} not shown) ===\n", crate::output::count(unseen, "cell")),
            budget,
        );
    }
    if other_cells > 0 {
        crate::explore::append_note(
            &mut head,
            &format!(
                "=== sheets 2-{} (0 of {} shown) ===\n",
                workbook.sheets.len(),
                crate::output::count(other_cells, "cell")
            ),
            budget,
        );
    }
    catalogue.append(&mut head, "sheets", budget);
    head.stopped = (head_budget < crate::explore::HEAD && head.stopped) || head.text.is_empty();
    Ok(head)
}

fn formula_value(formula: Result<String>, cache: &str) -> String {
    let formula = formula.and_then(|text| {
        anyhow::ensure!(!text.trim().is_empty(), "empty formula expression");
        Ok(text)
    });
    match formula {
        Ok(formula) => format!("{} → {cache}", crate::output::formula(&formula)),
        Err(error) => format!("(formula unavailable: {}) → {cache}", quoted(&format!("{error:#}"))),
    }
}

fn value(cell: &Cell, strings: &SharedStringTable, styles: Option<&StyleSheet>, date1904: bool) -> Result<String> {
    if !cell.cached {
        return Ok("(no cached value)".to_owned());
    }
    match cell.kind.as_str() {
        "s" => Ok(quoted(&crate::xstring::decode(
            strings.get(cell.value.parse()?).context("shared string index is absent")?,
        )?)),
        "inlineStr" | "str" => Ok(quoted(&crate::xstring::decode(&cell.value)?)),
        "b" => match cell.value.as_str() {
            "0" => Ok("false".into()),
            "1" => Ok("true".into()),
            _ => bail!("invalid Boolean at {}", cell.address),
        },
        "e" => Ok(format!("error {}", cell.value)),
        "d" => Ok(cell.value.replace('T', " ")),
        "" | "n" => {
            if cell.value.is_empty() {
                return Ok("(no cached value)".into());
            }
            let number: f64 = cell.value.parse().context("invalid numeric cell")?;
            // An omitted s attribute denotes XF zero.
            let index = cell.style.unwrap_or(0);
            let format = styles.and_then(|s| s.number_format_for(index));
            let id = styles.and_then(|s| s.number_format_id_for(index));
            Ok(crate::number::saved(&cell.value, number, format, id, date1904))
        }
        other => bail!("{}: unsupported cell type {other}", cell.address),
    }
}

fn scan(
    zip: &mut ZipArchive<File>,
    part: &str,
    mut cell: impl FnMut(Cell) -> Result<bool>,
    mut merge: impl FnMut(String) -> Result<()>,
) -> Result<bool> {
    let source = BufReader::new(zip.by_name(part)?);
    scan_xml(source, &mut cell, &mut merge).with_context(|| format!("reading {part}"))
}

fn scan_xml(
    source: impl BufRead,
    cell: &mut impl FnMut(Cell) -> Result<bool>,
    merge: &mut impl FnMut(String) -> Result<()>,
) -> Result<bool> {
    let mut reader = Reader::from_reader(source);
    reader.config_mut().expand_empty_elements = true;
    let mut buffer = Vec::new();
    let mut current: Option<Cell> = None;
    let mut field = String::new();
    let mut row = 0;
    let mut column = 0;
    let mut phonetic = false;
    let mut closed = false;
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(e) => match e.local_name().as_ref() {
                "row" => {
                    row = match attr(&e, "r")? {
                        Some(r) => r.parse()?,
                        None => row + 1,
                    };
                    column = 0;
                }
                "c" => {
                    let address = attr(&e, "r")?.unwrap_or_else(|| format!("{}{}", CellRef::col_name(column), row));
                    let parsed = CellRef::parse(&address).context("invalid cell reference")?;
                    column = parsed.col + 1;
                    current = Some(Cell {
                        address,
                        kind: attr(&e, "t")?.unwrap_or_default(),
                        style: attr(&e, "s")?.map(|s| s.parse()).transpose()?,
                        ..Default::default()
                    });
                }
                "f" if current.is_some() => {
                    let c = current.as_mut().unwrap();
                    c.formula = Some(String::new());
                    c.formula_kind = attr(&e, "t")?.unwrap_or_default();
                    c.shared = attr(&e, "si")?.map(|s| s.parse()).transpose()?;
                    c.formula_range = attr(&e, "ref")?;
                    field = "f".to_owned();
                }
                "v" | "t" if current.is_some() && !phonetic => {
                    current.as_mut().unwrap().cached = true;
                    field = e.local_name().as_ref().to_owned();
                }
                "rPh" => phonetic = true,
                "mergeCell" => merge(attr(&e, "ref")?.context("merge has no range")?)?,
                _ => {}
            },
            Event::End(e) => match e.local_name().as_ref() {
                "c" => {
                    if !cell(current.take().context("unmatched cell end")?)? {
                        return Ok(false);
                    }
                }
                "v" | "t" | "f" => field.clear(),
                "rPh" => phonetic = false,
                "worksheet" | "macrosheet" => closed = true,
                _ => {}
            },
            Event::Text(e) if !field.is_empty() => {
                append(&mut current, &field, &e.xml_content(quick_xml::XmlVersion::Implicit1_0))
            }
            Event::CData(e) if !field.is_empty() => append(&mut current, &field, e.as_ref()),
            Event::GeneralRef(e) if !field.is_empty() => append(&mut current, &field, &xml::resolve_general_ref(&e)?),
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !closed {
        bail!("worksheet XML ended before its closing tag");
    }
    Ok(true)
}

fn append(current: &mut Option<Cell>, field: &str, text: &str) {
    if let Some(cell) = current {
        if field == "f" {
            cell.formula.as_mut().unwrap().push_str(text);
        } else {
            cell.value.push_str(text);
        }
    }
}
