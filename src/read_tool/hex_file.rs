//! Sixteen-byte paged hexadecimal view for any regular file.

use super::DEFAULT_HEX_LINE_LIMIT;
use crate::budget::{TokenBudget, estimate_tokens};
use crate::head_note::{CoverageTotal, CoveredRange, HeadMetric, HeadNote};
use crate::model::ToolResponse;
use crate::paths::io_error_message;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const BYTES_PER_LINE: u64 = 16;
const HEX_COLUMN_WIDTH: usize = 48;

pub(super) fn read_hex_file(
    path: &Path,
    offset: Option<usize>,
    limit: Option<usize>,
    budget: TokenBudget,
) -> ToolResponse {
    let offset = offset.unwrap_or(1);
    let limit = limit.unwrap_or(DEFAULT_HEX_LINE_LIMIT);
    if offset == 0 {
        return ToolResponse::error("Invalid offset value: 0. Expected an integer >= 1.");
    }
    if limit == 0 {
        return ToolResponse::error("Invalid limit value: 0. Expected an integer >= 1.");
    }

    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) => return ToolResponse::error(io_error_message(path, &error)),
    };
    let file_size = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(error) => return ToolResponse::error(io_error_message(path, &error)),
    };
    if file_size == 0 {
        return HeadNote::new(
            crate::paths::display_path(path),
            HeadMetric::count(0, "line", "lines"),
        )
        .into_text_response("");
    }
    let total_lines = file_size / BYTES_PER_LINE + u64::from(file_size % BYTES_PER_LINE != 0);
    let offset_line = offset as u64;
    if offset_line > total_lines {
        return HeadNote::new(
            crate::paths::display_path(path),
            HeadMetric::count(0, "line", "lines"),
        )
        .fact(format!(
            "file has {total_lines} {}",
            if total_lines == 1 { "line" } else { "lines" }
        ))
        .into_text_response("");
    }

    let byte_offset = (offset_line - 1) * BYTES_PER_LINE;
    if let Err(error) = file.seek(SeekFrom::Start(byte_offset)) {
        return ToolResponse::error(io_error_message(path, &error));
    }
    let remaining = total_lines - offset_line + 1;
    let budget_probe = budget.value.saturating_mul(4).saturating_add(1) as u64;
    let candidate_lines = remaining.min(limit as u64).min(budget_probe.max(1));
    let mut rendered = Vec::with_capacity(candidate_lines.min(usize::MAX as u64) as usize);
    for line_index in 0..candidate_lines {
        let mut bytes = [0_u8; BYTES_PER_LINE as usize];
        let mut read = 0_usize;
        while read < bytes.len() {
            match file.read(&mut bytes[read..]) {
                Ok(0) => break,
                Ok(count) => read += count,
                Err(error) => return ToolResponse::error(io_error_message(path, &error)),
            }
        }
        if read == 0 {
            break;
        }
        rendered.push(format_hex_line(
            byte_offset + line_index * BYTES_PER_LINE,
            &bytes[..read],
        ));
    }

    loop {
        if rendered.is_empty() {
            return ToolResponse::error(format!(
                "{}={} is too small to return the response head note and one hex line. Increase it and retry.",
                budget.variable, budget.value
            ));
        }
        let shown = rendered.len() as u64;
        let last = offset_line + shown - 1;
        let note = HeadNote::new(
            crate::paths::display_path(path),
            HeadMetric::Coverage {
                unit: "lines",
                ranges: vec![CoveredRange::new(offset_line as usize, last as usize)],
                total: CoverageTotal::Exact(total_lines as usize),
            },
        );
        let output = note.render_with_body(&rendered.join("\n"));
        if estimate_tokens(&output) <= budget.value {
            return ToolResponse::text(output);
        }
        rendered.pop();
    }
}

fn format_hex_line(offset: u64, bytes: &[u8]) -> String {
    let mut hex_column = String::with_capacity(HEX_COLUMN_WIDTH);
    for index in 0..BYTES_PER_LINE as usize {
        if index > 0 {
            hex_column.push(' ');
        }
        if index == 8 {
            hex_column.push(' ');
        }
        if let Some(byte) = bytes.get(index) {
            let _ = write!(hex_column, "{byte:02x}");
        } else {
            hex_column.push_str("  ");
        }
    }
    debug_assert_eq!(hex_column.len(), HEX_COLUMN_WIDTH);
    let ascii = bytes
        .iter()
        .map(|byte| {
            if (0x20..=0x7E).contains(byte) {
                char::from(*byte)
            } else {
                '.'
            }
        })
        .collect::<String>();
    format!("{offset:08x}  {hex_column}  |{ascii}|")
}
