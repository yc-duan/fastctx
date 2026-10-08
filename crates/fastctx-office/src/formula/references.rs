use anyhow::{Result, ensure};
use office_oxide::xlsx::CellRef;

pub(super) fn cell(row: u16, column: u16, origin: (u16, u16), relative: bool) -> Result<String> {
    let (row, col) = coordinates(row, column, origin, relative)?;
    Ok(format!(
        "{}{}{}{}",
        if column & 0x4000 == 0 { "$" } else { "" },
        CellRef::col_name(col as u32),
        if column & 0x8000 == 0 { "$" } else { "" },
        u32::from(row) + 1
    ))
}

fn coordinates(row: u16, column: u16, origin: (u16, u16), relative: bool) -> Result<(u16, u16)> {
    let col = column & 0x3fff;
    if relative {
        let row = if column & 0x8000 != 0 { origin.0.wrapping_add(row) } else { row };
        let col = if column & 0x4000 != 0 {
            // BIFF8 signed relative offsets wrap in its 65536 x 256 grid.
            let offset = (i32::from(col) << 18) >> 18;
            ensure!((-255..=255).contains(&offset), "relative BIFF8 column offset outside -255..255");
            ((i32::from(origin.1) + offset) & 0xff) as u16
        } else {
            col
        };
        ensure!(col < 256, "BIFF8 column outside IV");
        Ok((row, col))
    } else {
        ensure!(col < 256, "BIFF8 column outside IV");
        Ok((row, col))
    }
}

pub(super) fn area(rows: (u16, u16), cols: (u16, u16), origin: (u16, u16), relative: bool) -> Result<String> {
    let (r1, c1) = coordinates(rows.0, cols.0, origin, relative)?;
    let (r2, c2) = coordinates(rows.1, cols.1, origin, relative)?;
    if r1 == 0 && r2 == u16::MAX && cols.0 & 0x8000 == 0 && cols.1 & 0x8000 == 0 {
        return Ok(format!(
            "{}{}:{}{}",
            if cols.0 & 0x4000 == 0 { "$" } else { "" },
            CellRef::col_name(c1 as u32),
            if cols.1 & 0x4000 == 0 { "$" } else { "" },
            CellRef::col_name(c2 as u32)
        ));
    }
    Ok(format!("{}:{}", cell(rows.0, cols.0, origin, relative)?, cell(rows.1, cols.1, origin, relative)?))
}

pub(super) fn sheet_name(name: &str) -> String {
    let simple = !name.is_empty()
        && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        && !name.chars().next().unwrap().is_ascii_digit()
        && CellRef::parse(name).is_none()
        && !name.eq_ignore_ascii_case("TRUE")
        && !name.eq_ignore_ascii_case("FALSE");
    if simple { name.to_owned() } else { format!("'{}'", name.replace('\'', "''")) }
}
