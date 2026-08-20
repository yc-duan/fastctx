//! Byte-level editing of the FastCtx-owned section in `~/.codex/AGENTS.md`.

use serde::{Deserialize, Serialize};

const BEGIN_MARKER: &str = "<!-- fastctx:begin -->";
const END_MARKER: &str = "<!-- fastctx:end -->";
pub(crate) const MANAGED_SECTION_CONTRACT_ID: &str = "guidance-v5";
const LEGACY_BEGIN_MARKER: &str = "<!-- fastread:begin -->";
const LEGACY_END_MARKER: &str = "<!-- fastread:end -->";
const LEGACY_FASTREAD_SECTION: &str = concat!(
    "<!-- fastread:begin -->\n",
    "## Local file inspection\n",
    "\n",
    "The fastread MCP tools are the first-class way to read, search, and find\n",
    "local files: `mcp__fastread__read`, `mcp__fastread__grep`,\n",
    "`mcp__fastread__glob` — prefer them over `cat`/`Get-Content`,\n",
    "`rg`/`findstr`/`Select-String`, and `dir`/`ls -R`. Pass absolute paths. The\n",
    "last line of every result says `Complete` or `Partial` — continue only with\n",
    "the exact parameters a `Partial` note provides.\n",
    "<!-- fastread:end -->"
);
// Tools are named as the server plus a bare tool name rather than as one mangled
// identifier. Hosts differ in how they spell an MCP tool — some flatten the server and
// the tool into a single name, others expose the server as a namespace whose members are
// the bare tool names — so any single mangled spelling names something at least one host
// has no entry for, while the server-plus-name form resolves under both (2026-08-08).
const FILE_GUIDANCE_PREFIX: &str = concat!(
    "## Local file inspection\n",
    "\n",
    "For reading, searching, and finding local files, prefer the FastCtx MCP\n",
    "server's own tools — `inspect_local_file`, `grep`, and `glob` — over shell\n",
    "equivalents such as `cat`/`Get-Content`, `rg`/`findstr`/`Select-String`,\n",
    "and `dir`/`ls -R`.\n",
);
const FILE_GUIDANCE_SUFFIX: &str = concat!(
    "Read only what the task needs. When you need several files, pass them to\n",
    "one `inspect_local_file` call as files=[{\"path\": ...}, ...] instead of one\n",
    "call per file. The last line of every result says `Complete` or\n",
    "`Partial` — continue only with the exact parameters a `Partial` note\n",
    "provides.\n",
    "\n",
    "### Batch replacement\n",
    "\n",
    "Use FastCtx's `replace` for mechanical find-and-replace across files.\n",
    "It preserves each file's encoding and line endings, supports dry-run previews,\n",
    "and rejects concurrent changes before writing. Use apply_patch for generated\n",
    "content, semantic rewrites, or small local edits.\n"
);
const SHELL_GUIDANCE: &str = concat!(
    "### Shell commands\n",
    "\n",
    "Prefer FastCtx's `run` over the built-in shell for terminal work: it\n",
    "executes with bash (Git Bash on Windows), so always write POSIX bash —\n",
    "never PowerShell syntax.\n",
    "\n",
    "Never pass `apply_patch` to FastCtx's `run`: it is not a program and\n",
    "no shell can run it. Reach it through Codex itself — as its own tool\n",
    "call, or in Codex's built-in shell — never through the FastCtx tools.\n",
    "\n",
    "Commands must be non-interactive (no TTY): use flags like -y\n",
    "or --no-edit, and expect editors/pagers to be disabled. For anything\n",
    "that may outlast run's four-minute maximum, use `run_background`, check\n",
    "on it with `job_output`, and stop it with `job_kill`. Background jobs run\n",
    "independently of this session and survive restarts; rediscover an earlier\n",
    "job with `job_list` and read its output by job_id. A non-zero exit code is\n",
    "a normal result. The last line of every result says `Complete` or\n",
    "`Partial`.\n"
);
// Byte-frozen guidance from superseded releases. This is comparison data only: it must
// never be emitted except when replacing an exact on-disk match with the current section.
//
// Every contract that ships has to land here when it is superseded. A product update
// refreshes the managed block only on an exact match against one of these, so a release
// that is left out strands its users with a block naming tools this build no longer
// publishes — and only `fastctx apply` would ever repair it.
const V022_RESOURCE_ROUTING_FILE_GUIDANCE: &str = concat!(
    "## Local file inspection\n",
    "\n",
    "For reading, searching, and finding local files, prefer the FastCtx MCP\n",
    "tools — `mcp__fastctx__read`, `mcp__fastctx__grep`, `mcp__fastctx__glob` —\n",
    "over `cat`/`Get-Content`, `rg`/`findstr`/`Select-String`, and `dir`/`ls -R`.\n",
    "Read only what the task needs. When you need several files, pass them to\n",
    "one read call as files=[{\"path\": ...}, ...] instead of one call per file.\n",
    "Pass absolute paths. The last line of every result says `Complete` or\n",
    "`Partial` — continue only with the exact parameters a `Partial` note\n",
    "provides.\n",
    "\n",
    "Never point `read_mcp_resource`, `list_mcp_resources`, or\n",
    "`list_mcp_resource_templates` at the `fastctx` server: FastCtx publishes\n",
    "tools, not MCP resources, so those calls always fail. Read a local file\n",
    "with `mcp__fastctx__read` and an absolute path — never a `file://` URI.\n",
    "\n",
    "### Batch replacement\n",
    "\n",
    "Use `mcp__fastctx__replace` for mechanical find-and-replace across files.\n",
    "It preserves each file's encoding and line endings, supports dry-run previews,\n",
    "and rejects concurrent changes before writing. Use apply_patch for generated\n",
    "content, semantic rewrites, or small local edits.\n"
);
const V022_RESOURCE_ROUTING_SHELL_GUIDANCE: &str = concat!(
    "### Shell commands\n",
    "\n",
    "Prefer `mcp__fastctx__run` over the built-in shell for terminal work: it\n",
    "executes with bash (Git Bash on Windows), so always write POSIX bash —\n",
    "never PowerShell syntax.\n",
    "\n",
    "Never pass `apply_patch` to `mcp__fastctx__run`: it is not a program and\n",
    "no shell can run it. Reach it through Codex itself — as its own tool\n",
    "call, or in Codex's built-in shell — never through the FastCtx tools.\n",
    "\n",
    "Commands must be non-interactive (no TTY): use flags like -y\n",
    "or --no-edit, and expect editors/pagers to be disabled. For anything\n",
    "that may outlast run's four-minute maximum, use\n",
    "`mcp__fastctx__run_background`, check on it with\n",
    "`mcp__fastctx__job_output`, and stop it with `mcp__fastctx__job_kill`.\n",
    "Background jobs run independently of this session and survive restarts;\n",
    "rediscover an earlier job with `mcp__fastctx__job_list` and read its\n",
    "output by job_id. A non-zero exit code is a normal result. The last line\n",
    "of every result says `Complete` or `Partial`.\n"
);
const V024_READ_TOOL_NAME_FILE_GUIDANCE: &str = concat!(
    "## Local file inspection\n",
    "\n",
    "For reading, searching, and finding local files, prefer the FastCtx MCP\n",
    "tools — `mcp__fastctx__read`, `mcp__fastctx__grep`, `mcp__fastctx__glob` —\n",
    "over `cat`/`Get-Content`, `rg`/`findstr`/`Select-String`, and `dir`/`ls -R`.\n",
    "Use FastCtx file tools directly for local-file operations, including when a\n",
    "local reference is URI-shaped; pass the equivalent plain absolute filesystem path.\n",
    "Read only what the task needs. When you need several files, pass them to\n",
    "one read call as files=[{\"path\": ...}, ...] instead of one call per file.\n",
    "The last line of every result says `Complete` or\n",
    "`Partial` — continue only with the exact parameters a `Partial` note\n",
    "provides.\n",
    "\n",
    "### Batch replacement\n",
    "\n",
    "Use `mcp__fastctx__replace` for mechanical find-and-replace across files.\n",
    "It preserves each file's encoding and line endings, supports dry-run previews,\n",
    "and rejects concurrent changes before writing. Use apply_patch for generated\n",
    "content, semantic rewrites, or small local edits.\n"
);
// 0.2.4 shipped the shell section unchanged from 0.2.2, so the two releases share these exact
// bytes. Keep the alias rather than a second copy: the frozen hashes below cover both names, and
// a copy would let one drift while the other stayed put.
const V024_READ_TOOL_NAME_SHELL_GUIDANCE: &str = V022_RESOURCE_ROUTING_SHELL_GUIDANCE;

/// One byte-frozen managed block from a superseded release.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KnownLegacyGuidance {
    /// 0.2.5, superseded when file-tool target fields became explicit host guidance.
    TargetFields,
    /// 0.2.2/0.2.3, whose prohibition named the very resource tools it steered away from.
    ResourceRouting,
    /// 0.2.4, superseded when the file-inspection tool was renamed away from `read`.
    ReadToolName,
}

impl KnownLegacyGuidance {
    /// Every superseded release this build still recognises, newest first.
    pub(crate) const ALL: [Self; 3] = [
        Self::TargetFields,
        Self::ReadToolName,
        Self::ResourceRouting,
    ];

    fn frozen_guidance(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::TargetFields => None,
            Self::ResourceRouting => Some((
                V022_RESOURCE_ROUTING_FILE_GUIDANCE,
                V022_RESOURCE_ROUTING_SHELL_GUIDANCE,
            )),
            Self::ReadToolName => Some((
                V024_READ_TOOL_NAME_FILE_GUIDANCE,
                V024_READ_TOOL_NAME_SHELL_GUIDANCE,
            )),
        }
    }

    /// Rebuilds this release's exact managed block for the optional shell group.
    pub(crate) fn section(self, fastshell_enabled: bool) -> String {
        let mut output = String::from(BEGIN_MARKER);
        output.push('\n');
        if self == Self::TargetFields {
            output.push_str(FILE_GUIDANCE_PREFIX);
            output.push_str(crate::model_guidance::LOCAL_FILE_ROUTE_GUIDANCE);
            output.push('\n');
            output.push_str(FILE_GUIDANCE_SUFFIX);
            if fastshell_enabled {
                output.push('\n');
                output.push_str(SHELL_GUIDANCE);
            }
        } else if let Some((file_guidance, shell_guidance)) = self.frozen_guidance() {
            output.push_str(file_guidance);
            if fastshell_enabled {
                output.push('\n');
                output.push_str(shell_guidance);
            }
        }
        output.push_str(END_MARKER);
        output
    }
}

/// Separator bytes inserted and therefore owned by Apply between user content and the private section.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InsertedSeparator {
    /// Add one LF after existing content that already ends in LF.
    Lf,
    /// Add one CRLF after existing content that already ends in CRLF.
    CrLf,
    /// Add two LFs after existing content with no trailing newline.
    LfLf,
}

impl InsertedSeparator {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Lf => b"\n",
            Self::CrLf => b"\r\n",
            Self::LfLf => b"\n\n",
        }
    }
}

pub(crate) struct SectionEdit {
    pub bytes: Vec<u8>,
    pub inserted_separator: Option<InsertedSeparator>,
}

/// Semantic state of the one managed section relative to the receipt's shell mode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ManagedSectionState {
    Current,
    KnownLegacy,
    Missing,
    Drifted,
    Malformed(String),
}

/// Builds the exact managed block for the optional shell group.
pub fn section(fastshell_enabled: bool) -> String {
    let mut output = String::from(BEGIN_MARKER);
    output.push('\n');
    output.push_str(FILE_GUIDANCE_PREFIX);
    output.push_str(crate::model_guidance::LOCAL_FILE_ROUTE_GUIDANCE);
    output.push('\n');
    output.push_str(crate::model_guidance::LOCAL_FILE_TARGET_FIELD_GUIDANCE);
    output.push('\n');
    output.push_str(FILE_GUIDANCE_SUFFIX);
    if fastshell_enabled {
        output.push('\n');
        output.push_str(SHELL_GUIDANCE);
    }
    output.push_str(END_MARKER);
    output
}

/// Idempotently inserts or replaces the FastCtx section without changing bytes outside it.
pub fn apply_section(original: &[u8]) -> Result<Vec<u8>, String> {
    Ok(apply_section_with_ownership(original)?.bytes)
}

/// Computes the private-section edit and returns ownership of any newly inserted leading separator.
pub(crate) fn apply_section_with_ownership(original: &[u8]) -> Result<SectionEdit, String> {
    apply_section_with_ownership_for(original, false)
}

/// Applies the exact managed block for the optional shell group.
pub(crate) fn apply_section_with_ownership_for(
    original: &[u8],
    fastshell_enabled: bool,
) -> Result<SectionEdit, String> {
    let original = remove_exact_legacy_section(original)?;
    let source = std::str::from_utf8(&original).map_err(|error| {
        format!(
            "Cannot edit AGENTS.md because it is not valid UTF-8 ({error}). Convert it to UTF-8 and retry."
        )
    })?;
    let expected = section(fastshell_enabled);
    match section_range(source)? {
        Some((start, end)) => {
            let mut output = Vec::with_capacity(original.len() + expected.len());
            output.extend_from_slice(&original[..start]);
            output.extend_from_slice(expected.as_bytes());
            output.extend_from_slice(&original[end..]);
            Ok(SectionEdit {
                bytes: output,
                inserted_separator: None,
            })
        }
        None => {
            let mut output = original;
            let mut inserted_separator = None;
            if !output.is_empty() {
                if output.ends_with(b"\r\n\r\n") || output.ends_with(b"\n\n") {
                    // An existing blank line already separates the content, so append the frozen section directly.
                } else if output.ends_with(b"\r\n") {
                    output.extend_from_slice(b"\r\n");
                    inserted_separator = Some(InsertedSeparator::CrLf);
                } else if output.ends_with(b"\n") {
                    output.push(b'\n');
                    inserted_separator = Some(InsertedSeparator::Lf);
                } else {
                    output.extend_from_slice(b"\n\n");
                    inserted_separator = Some(InsertedSeparator::LfLf);
                }
            }
            output.extend_from_slice(expected.as_bytes());
            output.push(b'\n');
            Ok(SectionEdit {
                bytes: output,
                inserted_separator,
            })
        }
    }
}

/// Removes the FastCtx section while preserving all other bytes.
pub fn remove_section(original: &[u8]) -> Result<Vec<u8>, String> {
    remove_applied_section(original, None)
}

/// Removes the private section and its recorded leading separator only when the Apply receipt proves no drift.
pub(crate) fn remove_applied_section(
    original: &[u8],
    inserted_separator: Option<InsertedSeparator>,
) -> Result<Vec<u8>, String> {
    let source = std::str::from_utf8(original).map_err(|error| {
        format!(
            "Cannot edit AGENTS.md because it is not valid UTF-8 ({error}). Convert it to UTF-8 and retry."
        )
    })?;
    let Some((start, mut end)) = section_range(source)? else {
        return Ok(original.to_vec());
    };
    let owned_start = inserted_separator
        .filter(|separator| original[..start].ends_with(separator.bytes()))
        .map_or(start, |separator| start - separator.bytes().len());
    if original.get(end..end + 2) == Some(b"\r\n") {
        end += 2;
    } else if original.get(end) == Some(&b'\n') {
        end += 1;
    }
    let mut output = Vec::with_capacity(original.len().saturating_sub(end - owned_start));
    output.extend_from_slice(&original[..owned_start]);
    output.extend_from_slice(&original[end..]);
    Ok(output)
}

/// Returns whether the managed section exists and exactly matches the current appendix contract.
pub fn has_exact_section(bytes: &[u8]) -> Result<bool, String> {
    has_exact_section_for(bytes, false)
}

/// Checks the managed block against the exact optional-shell state.
pub fn has_exact_section_for(bytes: &[u8], fastshell_enabled: bool) -> Result<bool, String> {
    let source = std::str::from_utf8(bytes)
        .map_err(|error| format!("AGENTS.md is not valid UTF-8: {error}"))?;
    let expected = section(fastshell_enabled);
    Ok(section_range(source)?
        .map(|(start, end)| source[start..end] == expected)
        .unwrap_or(false))
}

pub(crate) fn classify_managed_section(
    bytes: &[u8],
    fastshell_enabled: bool,
) -> ManagedSectionState {
    let source = match std::str::from_utf8(bytes) {
        Ok(source) => source,
        Err(error) => {
            return ManagedSectionState::Malformed(format!(
                "AGENTS.md is not valid UTF-8: {error}"
            ));
        }
    };
    let Some((start, end)) = (match section_range(source) {
        Ok(range) => range,
        Err(error) => return ManagedSectionState::Malformed(error),
    }) else {
        return ManagedSectionState::Missing;
    };
    let managed = &source[start..end];
    if managed == section(fastshell_enabled) {
        ManagedSectionState::Current
    } else if KnownLegacyGuidance::ALL
        .iter()
        .any(|legacy| managed == legacy.section(fastshell_enabled))
    {
        ManagedSectionState::KnownLegacy
    } else {
        ManagedSectionState::Drifted
    }
}

pub(crate) fn refresh_known_legacy_section(
    bytes: &[u8],
    fastshell_enabled: bool,
) -> Option<Vec<u8>> {
    if classify_managed_section(bytes, fastshell_enabled) != ManagedSectionState::KnownLegacy {
        return None;
    }
    let source = std::str::from_utf8(bytes).ok()?;
    let (start, end) = section_range(source).ok()??;
    let current = section(fastshell_enabled);
    let mut output = Vec::with_capacity(bytes.len() + current.len() - (end - start));
    output.extend_from_slice(&bytes[..start]);
    output.extend_from_slice(current.as_bytes());
    output.extend_from_slice(&bytes[end..]);
    Some(output)
}

fn section_range(source: &str) -> Result<Option<(usize, usize)>, String> {
    marker_range(
        source,
        BEGIN_MARKER,
        END_MARKER,
        "fastctx",
        "AGENTS.md contains duplicate or unmatched fastctx markers. Repair the marker block manually and retry.",
    )
}

fn remove_exact_legacy_section(original: &[u8]) -> Result<Vec<u8>, String> {
    let source = std::str::from_utf8(original).map_err(|error| {
        format!(
            "Cannot edit AGENTS.md because it is not valid UTF-8 ({error}). Convert it to UTF-8 and retry."
        )
    })?;
    let Some((start, mut end)) = marker_range(
        source,
        LEGACY_BEGIN_MARKER,
        LEGACY_END_MARKER,
        "fastread",
        "AGENTS.md contains duplicate or unmatched legacy fastread markers. Repair the marker block manually and retry.",
    )?
    else {
        return Ok(original.to_vec());
    };
    if &source[start..end] != LEGACY_FASTREAD_SECTION {
        return Ok(original.to_vec());
    }
    if original.get(end..end + 2) == Some(b"\r\n") {
        end += 2;
    } else if original.get(end) == Some(&b'\n') {
        end += 1;
    }
    let mut output = Vec::with_capacity(original.len().saturating_sub(end - start));
    output.extend_from_slice(&original[..start]);
    output.extend_from_slice(&original[end..]);
    Ok(output)
}

fn marker_range(
    source: &str,
    begin_marker: &str,
    end_marker: &str,
    label: &str,
    duplicate_message: &str,
) -> Result<Option<(usize, usize)>, String> {
    let begins = source.match_indices(begin_marker).collect::<Vec<_>>();
    let ends = source.match_indices(end_marker).collect::<Vec<_>>();
    if begins.is_empty() && ends.is_empty() {
        return Ok(None);
    }
    if begins.len() != 1 || ends.len() != 1 {
        return Err(duplicate_message.to_string());
    }
    let start = begins[0].0;
    let end_start = ends[0].0;
    if end_start < start {
        return Err(format!(
            "AGENTS.md has the {label} end marker before its begin marker. Repair the block manually and retry."
        ));
    }
    let end = end_start + end_marker.len();
    Ok(Some((start, end)))
}

#[cfg(test)]
mod tests {
    use super::{
        KnownLegacyGuidance, ManagedSectionState, classify_managed_section,
        refresh_known_legacy_section, section,
    };

    #[test]
    fn v025_guidance_is_recognized_and_refreshes_to_explicit_target_fields() {
        for fastshell_enabled in [false, true] {
            let legacy = KnownLegacyGuidance::TargetFields.section(fastshell_enabled);
            assert_eq!(
                classify_managed_section(legacy.as_bytes(), fastshell_enabled),
                ManagedSectionState::KnownLegacy
            );
            let refreshed = refresh_known_legacy_section(legacy.as_bytes(), fastshell_enabled)
                .expect("v0.2.5 guidance should refresh");
            assert_eq!(refreshed, section(fastshell_enabled).into_bytes());
            let refreshed = String::from_utf8(refreshed).unwrap();
            assert!(refreshed.contains("`inspect_local_file` uses `file_path`"));
            assert!(refreshed.contains("`grep`, `glob`, and `replace` use `path`"));
        }
    }
}
