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

pub(crate) fn read(file: File, selection: &Selection, budget: usize) -> Result<Content> {
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
    let mut out = Output::new(budget);
    out.reserve_facts(crate::spreadsheet::workbook_reserve());
    let (mut first, mut last) = (0, 0);
    for (index, sheet) in workbook.sheets.iter().enumerate() {
        if selected.is_some_and(|wanted| index != wanted) {
            continue;
        }
        let rel = rels.get_by_id(&sheet.rel_id).context("missing worksheet relationship")?;
        if !matches!(rel.rel_type.as_str(), rel_types::WORKSHEET | rel_types::MACROSHEET | rel_types::INTL_MACROSHEET) {
            if !out.line(
                "",
                &format!(
                    "=== sheet {} {} ({} content not shown) ===",
                    index + 1,
                    quoted(&sheet.name),
                    rel.rel_type.rsplit('/').next().unwrap_or(&rel.rel_type)
                ),
                &format!("use --sheet-index {}", index + 1),
            ) {
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
        let Some(header) = out.header(
            &format!("sheet {} {}", index + 1, quoted(&sheet.name)),
            &crate::spreadsheet::sheet_reserve(),
            &format!("use --sheet-index {}", index + 1),
        ) else {
            break;
        };
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
                    &format!("use --sheet-index {} --range {merge}", index + 1),
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
                    &format!("use --sheet-index {} --range {}:{}", index + 1, range.first, range.last),
                )
            {
                break;
            }
        }
        let (mut shown, mut through) = (0, None);
        if !out.stopped() {
            let mut array_cells = facts.arrays.cells(selection.range.as_ref());
            let mut emit = |address: CellRef, value: &str| {
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
        if out.stopped() {
            break;
        }
    }
    out.facts(crate::spreadsheet::workbook_facts(
        workbook.sheets.len(),
        first,
        last,
        selected.is_some() || out.stopped(),
    ));
    Ok(out.finish())
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
