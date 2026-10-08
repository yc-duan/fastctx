//! Locate BIFF FORMULA records independently of their cached values.
//! Calamine 0.36 drops PtgExp shared/array expressions as empty strings;
//! its sparse formula range alone cannot distinguish these from no formula.

use std::fs::File;
use std::io::{Seek, SeekFrom};

use anyhow::{Context, Result, bail};
use office_oxide::cfb::CfbReader;

pub(crate) struct FormulaLocation {
    pub row: usize,
    pub col: usize,
    pub shared_or_array: bool,
}

pub(crate) fn formula_cells(mut file: File) -> Result<Vec<Vec<FormulaLocation>>> {
    file.seek(SeekFrom::Start(0))?;
    let mut container = CfbReader::new(file)?;
    let name = if container.has_stream("Workbook") { "Workbook" } else { "Book" };
    let data = container.open_stream(name)?;
    let mut offsets = Vec::new();
    let mut at = 0;
    loop {
        let (kind, body) = record(&data, &mut at)?;
        match kind {
            0x0085 => {
                let bytes: [u8; 4] = body.get(..4).context("short BOUNDSHEET record")?.try_into()?;
                offsets.push(u32::from_le_bytes(bytes) as usize);
            }
            0x000a => break,
            _ => {}
        }
    }
    offsets
        .into_iter()
        .map(|mut at| {
            let mut cells = Vec::new();
            let mut depth = 0usize;
            loop {
                let (kind, body) = record(&data, &mut at)?;
                match kind {
                    0x0009 | 0x0209 | 0x0409 | 0x0809 => depth += 1,
                    0x0006 if depth == 1 => {
                        let bytes = body.get(..4).context("short FORMULA record")?;
                        cells.push(FormulaLocation {
                            row: u16::from_le_bytes([bytes[0], bytes[1]]) as usize,
                            col: u16::from_le_bytes([bytes[2], bytes[3]]) as usize,
                            // FORMULA's parsed expression starts after the
                            // 20-byte fixed fields and two-byte token count.
                            shared_or_array: body.get(22) == Some(&0x01),
                        });
                    }
                    0x000a => {
                        depth = depth.checked_sub(1).context("BIFF EOF without BOF")?;
                        if depth == 0 {
                            return Ok(cells);
                        }
                    }
                    _ => {}
                }
            }
        })
        .collect()
}

fn record<'a>(data: &'a [u8], at: &mut usize) -> Result<(u16, &'a [u8])> {
    let header = data.get(*at..).and_then(|s| s.get(..4)).context("BIFF stream ended before EOF")?;
    let kind = u16::from_le_bytes([header[0], header[1]]);
    let length = u16::from_le_bytes([header[2], header[3]]) as usize;
    *at += 4;
    if length > data.len() - *at {
        bail!("BIFF record extends past the workbook stream");
    }
    let body = &data[*at..*at + length];
    *at += length;
    Ok((kind, body))
}
