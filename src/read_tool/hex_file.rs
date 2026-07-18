//! Sixteen-byte paged hexadecimal view for any regular file.

use super::DEFAULT_LINE_LIMIT;
use crate::budget::{TokenBudget, assemble_text, estimate_tokens};
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
    let limit = limit.unwrap_or(DEFAULT_LINE_LIMIT);
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
        return ToolResponse::text("Warning: the file exists but is empty.");
    }
    let total_lines = file_size / BYTES_PER_LINE + u64::from(file_size % BYTES_PER_LINE != 0);
    let offset_line = offset as u64;
    if offset_line > total_lines {
        let noun = if total_lines == 1 { "line" } else { "lines" };
        return ToolResponse::text(format!(
            "Warning: the file has only {total_lines} {noun}, but offset={offset} was requested."
        ));
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
                "{}={} is too small to return the required continuation note. Increase it and retry.",
                budget.variable, budget.value
            ));
        }
        let shown = rendered.len() as u64;
        let last = offset_line + shown - 1;
        let terminal = if last < total_lines {
            format!(
                "(Partial: {} of {total_lines} shown. Continue with offset={}.)",
                line_span(offset_line, last),
                last + 1
            )
        } else {
            format!(
                "(Complete: reached end of file; {} of {total_lines} shown.)",
                line_span(offset_line, last)
            )
        };
        let output = assemble_text(&rendered, &[terminal]);
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

fn line_span(first: u64, last: u64) -> String {
    if first == last {
        format!("line {first}")
    } else {
        format!("lines {first}-{last}")
    }
}

#[cfg(test)]
mod tests {
    use super::{format_hex_line, read_hex_file};
    use crate::ToolContent;
    use crate::budget::TokenBudget;

    #[test]
    fn full_and_partial_lines_keep_the_ascii_column_aligned() {
        assert_eq!(
            format_hex_line(0, b"0123456789ABCDEF"),
            "00000000  30 31 32 33 34 35 36 37  38 39 41 42 43 44 45 46  |0123456789ABCDEF|"
        );
        assert_eq!(
            format_hex_line(16, &[0x20, 0x7E, 0x1F, 0x7F]),
            "00000010  20 7e 1f 7f                                       | ~..|"
        );
        assert_eq!(
            format_hex_line(0x1_0000_0000, b"x"),
            "100000000  78                                                |x|"
        );
    }

    #[test]
    fn token_budget_never_returns_an_unusable_success() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("bytes.bin");
        std::fs::write(&path, b"0123456789ABCDEFmore").unwrap();
        let response = read_hex_file(
            &path,
            None,
            None,
            TokenBudget {
                value: 1,
                variable: "FASTCTX_READ_TOKEN_BUDGET",
            },
        );
        assert!(response.is_error);
        assert_eq!(
            response.content,
            vec![ToolContent::Text(
                "FASTCTX_READ_TOKEN_BUDGET=1 is too small to return the required continuation note. Increase it and retry."
                    .to_string()
            )]
        );
    }
}
