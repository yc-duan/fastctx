//! Ownership-aware byte editor for DeepSeek Harness' machine patch file.
//!
//! DSH patch files are JavaScript-flavoured YAML and may contain values (such as
//! `!!js process.cwd()`) that a YAML round trip cannot preserve.  FastCtx therefore
//! owns one marker-delimited block and leaves every other byte untouched.

use crate::control::settings::{Tier, ToolBudgets};
use serde::{Deserialize, Serialize};
use std::ops::Range;

pub const BEGIN_MARKER: &str = "# fastctx:begin deepseek-harness";
pub const END_MARKER: &str = "# fastctx:end deepseek-harness";
pub const TOOL_TIMEOUT_MS: u64 = 300_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectedConfig {
    pub command: String,
    pub args: Vec<String>,
    pub tier: Tier,
    pub fastctx_budget: usize,
    pub tool_budgets: ToolBudgets,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockState {
    Missing,
    Current,
    Drifted,
    Malformed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Edit {
    pub bytes: Vec<u8>,
    pub state: BlockState,
    pub inserted_separator: Option<InsertedSeparator>,
}

/// Separator bytes inserted before a newly appended managed block.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InsertedSeparator {
    Lf,
    CrLf,
    LfLf,
    CrLfCrLf,
}

impl InsertedSeparator {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Lf => b"\n",
            Self::CrLf => b"\r\n",
            Self::LfLf => b"\n\n",
            Self::CrLfCrLf => b"\r\n\r\n",
        }
    }
}

fn marker_ranges(source: &str) -> Result<Option<Range<usize>>, String> {
    let begins = source
        .match_indices(BEGIN_MARKER)
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    let ends = source
        .match_indices(END_MARKER)
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    if begins.len() > 1 || ends.len() > 1 {
        return Err("DeepSeek Harness patch contains duplicate FastCtx markers.".to_string());
    }
    match (begins.first().copied(), ends.first().copied()) {
        (None, None) => Ok(None),
        (Some(_), None) | (None, Some(_)) => Err(
            "DeepSeek Harness patch contains an incomplete FastCtx marker block. Repair cordis.patch.yml manually and retry."
                .to_string(),
        ),
        (Some(begin), Some(end)) if end < begin => Err(
            "DeepSeek Harness patch contains reversed FastCtx markers. Repair cordis.patch.yml manually and retry."
                .to_string(),
        ),
        (Some(begin), Some(end)) => {
            let start = line_start(source, begin);
            let end = line_end(source, end);
            Ok(Some(start..end))
        }
    }
}

fn line_start(source: &str, offset: usize) -> usize {
    source[..offset].rfind('\n').map_or_else(
        || usize::from(source.as_bytes().starts_with(&[0xef, 0xbb, 0xbf])) * 3,
        |i| i + 1,
    )
}

fn line_end(source: &str, offset: usize) -> usize {
    source[offset..].find('\n').map_or(source.len(), |i| {
        let lf = offset + i;
        if lf > 0 && source.as_bytes()[lf - 1] == b'\r' {
            lf - 1
        } else {
            lf
        }
    })
}

fn newline(source: &[u8]) -> &'static [u8] {
    if source.windows(2).any(|window| window == b"\r\n") {
        b"\r\n"
    } else {
        b"\n"
    }
}

fn yaml_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn budget(global: usize, level: crate::control::settings::ToolBudgetLevel) -> Option<usize> {
    level.resolve(global)
}

fn block(expected: &ExpectedConfig, eol: &[u8]) -> Vec<u8> {
    let eol = std::str::from_utf8(eol).unwrap_or("\n");
    let mut lines = vec![
        BEGIN_MARKER.to_string(),
        "- insert:".to_string(),
        "    - id: mcp-fastctx".to_string(),
        "      name: '@deepseek-ai/dsh-mcp-client'".to_string(),
        "      config:".to_string(),
        "        serverName: fastctx".to_string(),
        "        transport: stdio".to_string(),
        format!("        command: {}", yaml_quote(&expected.command)),
        format!(
            "        args: [{}]",
            expected
                .args
                .iter()
                .map(|arg| yaml_quote(arg))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        "        env:".to_string(),
        format!(
            "          FASTCTX_TOKEN_BUDGET: {}",
            yaml_quote(&expected.fastctx_budget.to_string())
        ),
    ];
    for (name, level) in [
        ("FASTCTX_READ_TOKEN_BUDGET", expected.tool_budgets.read),
        ("FASTCTX_GREP_TOKEN_BUDGET", expected.tool_budgets.grep),
        ("FASTCTX_GLOB_TOKEN_BUDGET", expected.tool_budgets.glob),
        ("FASTCTX_RUN_TOKEN_BUDGET", expected.tool_budgets.run),
        (
            "FASTCTX_JOB_OUTPUT_TOKEN_BUDGET",
            expected.tool_budgets.job_output,
        ),
    ] {
        if let Some(value) = budget(expected.fastctx_budget, level) {
            lines.push(format!(
                "          {name}: {}",
                yaml_quote(&value.to_string())
            ));
        }
    }
    lines.extend([
        "        cwd: !!js process.cwd()".to_string(),
        format!("        toolCallTimeoutMs: {TOOL_TIMEOUT_MS}"),
        END_MARKER.to_string(),
    ]);
    lines.join(eol).into_bytes()
}

fn has_unmanaged_conflict(source: &str, managed: Option<&Range<usize>>) -> Option<&'static str> {
    for (needle, message) in [
        ("id: mcp-fastctx", "id: mcp-fastctx"),
        ("serverName: fastctx", "serverName: fastctx"),
    ] {
        let mut start = 0;
        while let Some(relative) = source[start..].find(needle) {
            let index = start + relative;
            if managed.is_none_or(|range| index < range.start || index >= range.end) {
                return Some(message);
            }
            start = index + needle.len();
        }
    }
    None
}

pub fn classify(original: &[u8], expected: &ExpectedConfig) -> Result<BlockState, String> {
    let source = std::str::from_utf8(original)
        .map_err(|error| format!("DeepSeek Harness patch is not valid UTF-8: {error}"))?;
    let range = match marker_ranges(source) {
        Ok(range) => range,
        Err(error) => return Ok(BlockState::Malformed(error)),
    };
    if let Some(conflict) = has_unmanaged_conflict(source, range.as_ref()) {
        return Err(format!(
            "DeepSeek Harness patch has an unmanaged FastCtx conflict ({conflict})."
        ));
    }
    let Some(range) = range else {
        return Ok(BlockState::Missing);
    };
    let expected = block(expected, newline(original));
    if source.as_bytes().get(range.clone()) == Some(expected.as_slice()) {
        Ok(BlockState::Current)
    } else {
        Ok(BlockState::Drifted)
    }
}

pub fn apply(original: &[u8], expected: &ExpectedConfig) -> Result<Edit, String> {
    let source = std::str::from_utf8(original)
        .map_err(|error| format!("DeepSeek Harness patch is not valid UTF-8: {error}"))?;
    let range = marker_ranges(source)?;
    if let Some(conflict) = has_unmanaged_conflict(source, range.as_ref()) {
        return Err(format!(
            "DeepSeek Harness patch has an unmanaged FastCtx conflict ({conflict})."
        ));
    }
    let eol = newline(original);
    let rendered = block(expected, eol);
    let (bytes, state, inserted_separator) = match range {
        Some(range) => {
            let current = &original[range.clone()];
            let state = if current == rendered.as_slice() {
                BlockState::Current
            } else {
                BlockState::Drifted
            };
            let mut output = Vec::with_capacity(original.len() + rendered.len());
            output.extend_from_slice(&original[..range.start]);
            output.extend_from_slice(&rendered);
            output.extend_from_slice(&original[range.end..]);
            (output, state, None)
        }
        None => {
            let mut output = original.to_vec();
            let inserted_separator = if output.is_empty() {
                None
            } else if output.ends_with(eol) {
                output.extend_from_slice(eol);
                Some(if eol == b"\r\n" {
                    InsertedSeparator::CrLf
                } else {
                    InsertedSeparator::Lf
                })
            } else {
                output.extend_from_slice(eol);
                output.extend_from_slice(eol);
                Some(if eol == b"\r\n" {
                    InsertedSeparator::CrLfCrLf
                } else {
                    InsertedSeparator::LfLf
                })
            };
            output.extend_from_slice(&rendered);
            (output, BlockState::Missing, inserted_separator)
        }
    };
    Ok(Edit {
        bytes,
        state,
        inserted_separator,
    })
}

pub fn remove(
    original: &[u8],
    expected: &ExpectedConfig,
    inserted_separator: Option<InsertedSeparator>,
) -> Result<Vec<u8>, String> {
    let source = std::str::from_utf8(original)
        .map_err(|error| format!("DeepSeek Harness patch is not valid UTF-8: {error}"))?;
    let Some(range) = marker_ranges(source)? else {
        return Ok(original.to_vec());
    };
    let rendered = block(expected, newline(original));
    if original.get(range.clone()) != Some(rendered.as_slice()) {
        return Err("DeepSeek Harness FastCtx block has drifted; refusing to remove user changes. Re-apply or repair cordis.patch.yml first.".to_string());
    }
    let start = inserted_separator
        .filter(|separator| original[..range.start].ends_with(separator.bytes()))
        .map_or(range.start, |separator| {
            range.start - separator.bytes().len()
        });
    let end = range.end;
    let mut output = Vec::with_capacity(original.len() - (end - start));
    output.extend_from_slice(&original[..start]);
    output.extend_from_slice(&original[end..]);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::settings::{Tier, ToolBudgetPreferences};

    fn expected() -> ExpectedConfig {
        ExpectedConfig {
            command: r"C:\Users\name\.fastctx\bin\fastctx.exe".into(),
            args: vec!["serve".into(), "--enable-shell".into()],
            tier: Tier::Standard,
            fastctx_budget: 54_000,
            tool_budgets: ToolBudgetPreferences::default().resolve(Tier::Standard),
        }
    }

    #[test]
    fn preserves_unmanaged_content_and_is_idempotent() {
        let original = b"# user\n- insert: custom\n";
        let first = apply(original, &expected()).unwrap();
        let second = apply(&first.bytes, &expected()).unwrap();
        assert_eq!(first.bytes, second.bytes);
        assert!(
            String::from_utf8(first.bytes)
                .unwrap()
                .starts_with("# user\n- insert: custom\n")
        );
    }

    #[test]
    fn detects_drift_and_refuses_remove() {
        let first = apply(b"", &expected()).unwrap();
        let mut changed = first.bytes;
        changed.extend_from_slice(b"# changed\n");
        assert!(remove(&changed, &expected(), None).is_err());
    }

    #[test]
    fn quotes_single_quotes() {
        let mut value = expected();
        value.command = "C:\\it's\\fastctx.exe".into();
        let edit = apply(b"", &value).unwrap();
        assert!(
            String::from_utf8(edit.bytes)
                .unwrap()
                .contains("C:\\it''s\\fastctx.exe")
        );
    }

    #[test]
    fn preserves_bom_and_crlf() {
        let original = b"\xef\xbb\xbf# user\r\n";
        let edit = apply(original, &expected()).unwrap();
        assert!(edit.bytes.starts_with(&[0xef, 0xbb, 0xbf]));
        assert!(!edit.bytes.windows(2).any(|window| window == b"\n\n"));
        let removed = remove(&edit.bytes, &expected(), edit.inserted_separator).unwrap();
        assert_eq!(removed, original);
    }

    #[test]
    fn marker_corruption_is_rejected_without_bytes() {
        for source in [
            format!("{BEGIN_MARKER}\nvalue\n"),
            format!("{END_MARKER}\n"),
            format!("{END_MARKER}\n{BEGIN_MARKER}\n"),
            format!("{BEGIN_MARKER}\n{BEGIN_MARKER}\n{END_MARKER}\n"),
            format!("{BEGIN_MARKER}\n{END_MARKER}\n{END_MARKER}\n"),
        ] {
            assert!(apply(source.as_bytes(), &expected()).is_err(), "{source:?}");
        }
    }

    #[test]
    fn unmanaged_fastctx_identity_conflicts_are_rejected() {
        for source in [
            b"- insert:\n    - id: mcp-fastctx\n".as_slice(),
            b"config:\n  serverName: fastctx\n".as_slice(),
        ] {
            let error = apply(source, &expected()).unwrap_err();
            assert!(error.contains("unmanaged FastCtx conflict"), "{error}");
        }
    }

    #[test]
    fn no_trailing_newline_round_trips_exactly() {
        let original = b"# user content";
        let edit = apply(original, &expected()).unwrap();
        let removed = remove(&edit.bytes, &expected(), edit.inserted_separator).unwrap();
        assert_eq!(removed, original);
    }

    #[test]
    fn config_changes_replace_only_the_managed_block() {
        let original = b"# before\ncustom: true\n";
        let first = apply(original, &expected()).unwrap();
        let mut changed = expected();
        changed.args = vec!["serve".into()];
        changed.fastctx_budget = 90_000;
        changed.tier = Tier::High;
        let second = apply(&first.bytes, &changed).unwrap();
        let text = String::from_utf8(second.bytes).unwrap();
        assert!(text.starts_with("# before\ncustom: true\n"));
        assert!(text.contains("args: ['serve']"));
        assert!(text.contains("FASTCTX_TOKEN_BUDGET: '90000'"));
        assert_eq!(text.matches(BEGIN_MARKER).count(), 1);
        assert_eq!(text.matches("toolCallTimeoutMs: 300000").count(), 1);
    }
}
