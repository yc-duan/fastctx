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

#[derive(Default)]
struct SheetFacts {
    merges: Vec<String>,
    masters: HashMap<u32, (CellRef, String)>,
    cells: usize,
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
            // XML 1.0 normalizes literal CR/CRLF before entity expansion.
            // oxide's SST reader retains those bytes; doing this on its
            // resulting text would also change intentional &#13; values.
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
    if !out.line(
        "",
        &format!("=== Excel ({} sheets; formula values are saved caches) ===", workbook.sheets.len()),
        "use --sheet-index 1",
    ) {
        return Ok(out.finish());
    }
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
            continue;
        }
        let part = package::target(&workbook_path, rel)?;
        let mut facts = SheetFacts::default();
        scan(
            &mut zip,
            &part,
            |cell| {
                if cell.cached || cell.formula.is_some() {
                    facts.cells += 1;
                }
                if cell.formula_kind == "shared" && cell.formula.as_ref().is_some_and(|f| !f.is_empty()) {
                    let id = cell.shared.context("shared formula has no index")?;
                    let address = CellRef::parse(&cell.address).context("invalid formula address")?;
                    if facts.masters.insert(id, (address, cell.formula.unwrap())).is_some() {
                        bail!("duplicate shared formula index {id}");
                    }
                }
                Ok(true)
            },
            |merge| {
                facts.merges.push(merge);
                Ok(())
            },
        )?;
        let state = match sheet.state {
            SheetState::Visible => "",
            SheetState::Hidden => "; hidden",
            SheetState::VeryHidden => "; very hidden",
        };
        if !out.line(
            "",
            &format!("=== sheet {} {} ({} stored cells{state}) ===", index + 1, quoted(&sheet.name), facts.cells),
            &format!("use --sheet-index {}", index + 1),
        ) {
            break;
        }
        for merge in &facts.merges {
            let merged: CellRange = merge.parse().context("invalid merged cell range")?;
            if selection.range.as_ref().is_none_or(|range| range.intersects(&merged))
                && !out.line("", &format!("merge {merge}"), &format!("use --sheet-index {} --range {merge}", index + 1))
            {
                return Ok(out.finish());
            }
        }
        let completed = scan(
            &mut zip,
            &part,
            |cell| {
                let address = CellRef::parse(&cell.address).context("invalid cell address")?;
                if selection.range.as_ref().is_some_and(|range| !range.contains(&address)) {
                    return Ok(true);
                }
                if !cell.cached && cell.formula.is_none() {
                    return Ok(true);
                }
                let mut value = value(&cell, &strings, styles.as_ref(), workbook.date1904)?;
                if let Some(formula) = &cell.formula {
                    let formula = if cell.formula_kind == "shared" && formula.is_empty() {
                        let id = cell.shared.context("shared formula has no index")?;
                        let (from, master) =
                            facts.masters.get(&id).with_context(|| format!("missing shared formula master {id}"))?;
                        translate(master, from, &address)
                    } else {
                        formula.clone()
                    };
                    let annotation = match cell.formula_kind.as_str() {
                        "array" => {
                            format!(" [array {}]", cell.formula_range.as_deref().context("array formula has no range")?)
                        }
                        "dataTable" => bail!("{address}: data-table formula parameters are not supported"),
                        "" | "normal" | "shared" => String::new(),
                        other => bail!("{address}: unsupported formula type {other}"),
                    };
                    value = format!("={formula}{annotation}\t=> {value}");
                }
                Ok(out.line(&format!("{address}\t"), &value, &resume(index, &address, selection.range.as_ref())))
            },
            |_| Ok(()),
        )?;
        if !completed {
            break;
        }
    }
    Ok(out.finish())
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
        "d" => Ok(format!("date {}", quoted(&cell.value))),
        "" | "n" => {
            if cell.value.is_empty() {
                return Ok("(no cached value)".into());
            }
            let _: f64 = cell.value.parse().context("invalid numeric cell")?;
            let mut out = cell.value.clone();
            if let (Some(styles), Some(index)) = (styles, cell.style)
                && let Some(format) = styles.number_format_for(index)
                && format != "General"
            {
                out.push_str(&format!("; format={}", quoted(format)));
                if office_oxide::xlsx::date::is_date_cell(cell.style, Some(styles)) {
                    out.push_str(if date1904 { "; date-system=1904" } else { "; date-system=1900" });
                }
            }
            Ok(out)
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

/// Quoted UTF-8 is copied bytewise by oxide 0.1.13's shared-formula
/// translator. Translate only the unquoted spans and retain literals exactly.
fn translate(formula: &str, from: &CellRef, to: &CellRef) -> String {
    let mut out = String::new();
    let mut start = 0;
    let mut chars = formula.char_indices().peekable();
    while let Some((at, quote)) = chars.next() {
        if quote != '\'' && quote != '"' {
            continue;
        }
        out.push_str(&office_oxide::xlsx::shared_formula::translate(&formula[start..at], from, to));
        let mut end = formula.len();
        while let Some((i, c)) = chars.next() {
            if c == quote {
                if chars.peek().is_some_and(|(_, c)| *c == quote) {
                    chars.next();
                } else {
                    end = i + c.len_utf8();
                    break;
                }
            }
        }
        out.push_str(&formula[at..end]);
        start = end;
    }
    out.push_str(&office_oxide::xlsx::shared_formula::translate(&formula[start..], from, to));
    out
}
