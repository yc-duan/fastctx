//! BIFF metadata and sparse formulas borrowing the workbook stream.
use crate::formula::{self, Book, Bytes, Context};
use anyhow::{Context as _, Result, bail, ensure};
use office_oxide::cfb::CfbReader;
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{Seek, SeekFrom};

pub(crate) struct Workbook {
    data: Vec<u8>,
    offsets: Vec<usize>,
    context: Context,
    version: u16,
    formats: HashMap<u16, String>,
    styles: Vec<u16>,
    pub date1904: bool,
}
#[derive(Clone, Copy)]
struct Expression<'a> {
    tokens: &'a [u8],
    extra: &'a [u8],
}
struct Group<'a> {
    first: (u16, u16),
    last: (u16, u16),
    shared: bool,
    expression: Expression<'a>,
}
pub(crate) struct Sheet<'a> {
    cells: BTreeMap<(u16, u16), std::result::Result<Expression<'a>, String>>,
    groups: HashMap<(u16, u16), std::result::Result<Group<'a>, String>>,
    strings: BTreeMap<(u16, u16), std::result::Result<String, String>>,
    workbook: &'a Workbook,
}
impl Workbook {
    pub fn read(mut file: File, names: Vec<String>) -> Result<Self> {
        file.seek(SeekFrom::Start(0))?;
        let mut container = CfbReader::new(file)?;
        let name = if container.has_stream("Workbook") { "Workbook" } else { "Book" };
        let data = container.open_stream(name)?;
        let mut result = Self {
            data,
            offsets: Vec::new(),
            context: Context { sheets: names, ..Default::default() },
            version: 0,
            formats: HashMap::new(),
            styles: Vec::new(),
            date1904: false,
        };
        let mut at = 0;
        loop {
            let (kind, body) = record(&result.data, &mut at)?;
            let mut input = Bytes(body);
            match kind {
                0x0809 => result.version = input.u16()?,
                0x0085 => result.offsets.push(input.u32()? as usize),
                0x0022 => result.date1904 = input.u16()? != 0,
                0x00e0 => {
                    input.take(2)?;
                    result.styles.push(input.u16()?);
                }
                0x041e if result.version == 0x0600 => {
                    let index = input.u16()?;
                    result.formats.insert(index, input.long_string()?);
                }
                0x0018 if result.version == 0x0600 => {
                    let flags = input.u16()?;
                    input.take(1)?;
                    let count = input.u8()? as usize;
                    input.take(10)?;
                    let name = input.string(count).and_then(|name| {
                        if flags & 0x20 == 0 {
                            return Ok(name);
                        }
                        let id = name.chars().next().context("empty built-in name")? as usize;
                        [
                            "Consolidate_Area",
                            "Auto_Open",
                            "Auto_Close",
                            "Extract",
                            "Database",
                            "Criteria",
                            "Print_Area",
                            "Print_Titles",
                            "Recorder",
                            "Data_Form",
                            "Auto_Activate",
                            "Auto_Deactivate",
                            "Sheet_Title",
                            "_FilterDatabase",
                        ]
                        .get(id)
                        .map(|s| s.to_string())
                        .context("unknown built-in name")
                    });
                    result.context.names.push(name.map_err(|e| e.to_string()));
                }
                0x01ae => {
                    let count = input.u16()?;
                    let marker = input.u16()?;
                    result.context.books.push(match marker {
                        0x0401 => Book::Internal,
                        0x3a01 => Book::Addin(Vec::new()),
                        length => {
                            let file = input.string(length as usize)?;
                            let sheets = (0..count).map(|_| input.long_string()).collect::<Result<Vec<_>>>()?;
                            Book::External { file, sheets }
                        }
                    });
                }
                0x0023 => {
                    if let Some(Book::Addin(names)) = result.context.books.last_mut() {
                        input.take(6)?;
                        let count = input.u8()? as usize;
                        names.push(input.string(count)?);
                    }
                }
                0x0017 if result.version == 0x0600 => {
                    let count = input.u16()?;
                    for _ in 0..count {
                        result.context.xtis.push((input.u16()?, input.u16()?, input.u16()?));
                    }
                }
                0x000a => break,
                _ => {}
            }
        }
        ensure!(
            result.offsets.len() == result.context.sheets.len(),
            "BIFF sheet directory and parsed workbook disagree"
        );
        Ok(result)
    }
    pub fn number_format(&self, style: u16) -> Result<(u32, Option<&str>)> {
        let id = *self.styles.get(style as usize).context("missing XF number format")?;
        Ok((
            id.into(),
            self.formats
                .get(&id)
                .map(String::as_str)
                .or_else(|| office_oxide::xlsx::numfmt::builtin_format_code(id as u32)),
        ))
    }
    pub fn sheet(&self, index: usize) -> Result<Sheet<'_>> {
        let mut at = *self.offsets.get(index).context("missing sheet stream")?;
        let mut sheet =
            Sheet { cells: BTreeMap::new(), groups: HashMap::new(), strings: BTreeMap::new(), workbook: self };
        let mut depth = 0usize;
        let mut previous = None;
        let mut pending_string = None;
        loop {
            let (kind, body) = record(&self.data, &mut at)?;
            if !matches!(kind, 0x0207 | 0x04bc | 0x0221 | 0x0236) {
                pending_string = None;
            }
            match kind {
                0x0009 | 0x0209 | 0x0409 | 0x0809 => depth += 1,
                0x0006 if depth == 1 => {
                    let mut input = Bytes(body);
                    let address = (input.u16()?, input.u16()?);
                    previous = Some(address);
                    if self.version == 0x0600 {
                        let cache = body.get(6..14).context("truncated formula cache")?;
                        if cache[6..8] == [0xff, 0xff] {
                            match cache[0] {
                                0 => {
                                    pending_string = Some(address);
                                    sheet.strings.insert(address, Err("missing STRING cache record".into()));
                                }
                                3 => {
                                    sheet.strings.insert(address, Ok(String::new()));
                                }
                                _ => {}
                            }
                        }
                    }
                    let parsed = (|| {
                        ensure!(
                            self.version == 0x0600,
                            "formula decoding requires BIFF8; file uses 0x{:04X}",
                            self.version
                        );
                        input.take(16)?;
                        expression(input.0)
                    })()
                    .map_err(|error| format!("{error:#}"));
                    sheet.cells.insert(address, parsed);
                }
                0x0207 if depth == 1 && pending_string.is_some() => {
                    let mut parts = vec![body];
                    while self.data.get(at..at + 2) == Some(&[0x3c, 0x00]) {
                        let (_, part) = record(&self.data, &mut at)?;
                        parts.push(part);
                    }
                    let value = crate::biff_string::read(&parts).map_err(|e| format!("{e:#}"));
                    sheet.strings.insert(pending_string.take().unwrap(), value);
                }
                0x04bc | 0x0221 if depth == 1 => {
                    let anchor = previous.context("formula group has no preceding FORMULA cell")?;
                    let group = (|| {
                        let mut input = Bytes(body);
                        let rows = (input.u16()?, input.u16()?);
                        let cols = (u16::from(input.u8()?), u16::from(input.u8()?));
                        input.take(if kind == 0x04bc { 2 } else { 6 })?;
                        Ok(Group {
                            first: (rows.0, cols.0),
                            last: (rows.1, cols.1),
                            shared: kind == 0x04bc,
                            expression: expression(input.0)?,
                        })
                    })()
                    .map_err(|error: anyhow::Error| format!("{error:#}"));
                    sheet
                        .groups
                        .entry(anchor)
                        .and_modify(|value| *value = Err("duplicate shared/array formula definitions".into()))
                        .or_insert(group);
                }
                0x000a => {
                    depth = depth.checked_sub(1).context("BIFF EOF without BOF")?;
                    if depth == 0 {
                        return Ok(sheet);
                    }
                }
                _ => {}
            }
        }
    }
}
impl Sheet<'_> {
    pub fn string_cache(&self, address: (u16, u16)) -> Option<std::result::Result<&str, &str>> {
        self.strings.get(&address).map(|v| v.as_ref().map(String::as_str).map_err(String::as_str))
    }
    pub fn array_ranges(&self) -> Vec<crate::spreadsheet::CellRange> {
        let mut ranges: Vec<_> = self
            .groups
            .values()
            .filter_map(|group| {
                let group = group.as_ref().ok()?;
                (!group.shared).then(|| crate::spreadsheet::CellRange {
                    first: office_oxide::xlsx::CellRef { row: group.first.0.into(), col: group.first.1.into() },
                    last: office_oxide::xlsx::CellRef { row: group.last.0.into(), col: group.last.1.into() },
                })
            })
            .collect();
        ranges.sort_by_key(|range| (range.first.row, range.first.col));
        ranges
    }
    pub fn positions(&self) -> impl Iterator<Item = (u16, u16)> + '_ {
        self.cells.keys().copied()
    }
    pub fn formula(&self, address: (u16, u16)) -> Option<Result<String>> {
        self.cells.get(&address).map(|value| {
            let mut expr = *value.as_ref().map_err(|reason| anyhow::anyhow!(reason.clone()))?;
            let mut shared = false;
            if expr.tokens.first() == Some(&0x01) {
                let mut input = Bytes(&expr.tokens[1..]);
                let anchor = (input.u16()?, input.u16()?);
                ensure!(input.0.is_empty(), "extra tokens after PtgExp");
                let group = self
                    .groups
                    .get(&anchor)
                    .context("missing shared/array formula definition")?
                    .as_ref()
                    .map_err(|reason| anyhow::anyhow!(reason.clone()))?;
                ensure!(
                    address.0 >= group.first.0
                        && address.0 <= group.last.0
                        && address.1 >= group.first.1
                        && address.1 <= group.last.1,
                    "cell is outside its formula group"
                );
                expr = group.expression;
                shared = group.shared;
            }
            formula::render(expr.tokens, expr.extra, address, shared, &self.workbook.context)
        })
    }
}
fn expression(bytes: &[u8]) -> Result<Expression<'_>> {
    let mut input = Bytes(bytes);
    let length = input.u16()? as usize;
    let tokens = input.take(length)?;
    ensure!(!tokens.is_empty(), "empty formula token stream");
    Ok(Expression { tokens, extra: input.0 })
}
fn record<'a>(data: &'a [u8], at: &mut usize) -> Result<(u16, &'a [u8])> {
    let header = data.get(*at..).and_then(|s| s.get(..4)).context("BIFF stream ended before EOF")?;
    let kind = u16::from_le_bytes([header[0], header[1]]);
    let length = u16::from_le_bytes([header[2], header[3]]) as usize;
    *at += 4;
    if length > data.len() - *at {
        bail!("BIFF record extends past workbook stream");
    }
    let body = &data[*at..*at + length];
    *at += length;
    Ok((kind, body))
}
