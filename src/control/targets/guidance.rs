//! Deterministic enabled-set guidance with marker and whole-file ownership modes.

use super::{AgentTarget, GuidanceKind};
use crate::control::agents::InsertedSeparator;
use crate::server_manifest::EnabledTools;
use sha2::{Digest, Sha256};

const BEGIN: &str = "<!-- fastctx:begin -->";
const END: &str = "<!-- fastctx:end -->";

pub(crate) struct GuidanceEdit {
    pub bytes: Vec<u8>,
    pub managed_hash: String,
    pub inserted_separator: Option<InsertedSeparator>,
}

pub(crate) fn apply_guidance(
    target: AgentTarget,
    original: Option<&[u8]>,
    tools: EnabledTools,
    owned: bool,
) -> Result<GuidanceEdit, String> {
    let generated = generated_guidance(target, tools);
    match target.guidance_kind() {
        GuidanceKind::SharedMarkdown => {
            apply_shared(original.unwrap_or_default(), &generated, owned)
        }
        GuidanceKind::CursorRule | GuidanceKind::CopilotInstructions | GuidanceKind::TraeRule => {
            apply_dedicated(target, original, &generated, owned)
        }
    }
}

pub(crate) fn disconnect_guidance(
    target: AgentTarget,
    original: &[u8],
    managed_hash: &str,
    inserted_separator: Option<InsertedSeparator>,
    original_existed: bool,
) -> Result<Option<Vec<u8>>, String> {
    match target.guidance_kind() {
        GuidanceKind::SharedMarkdown => {
            let source = utf8(original)?;
            let (start, mut end) = section_range(source)?.ok_or_else(|| {
                "The managed FastCtx guidance block is missing; FastCtx will not use stale ownership evidence."
                    .to_string()
            })?;
            if sha256(&original[start..end]) != managed_hash {
                return Err(
                    "The managed FastCtx guidance block drifted after Apply; FastCtx will not delete user-changed bytes."
                        .to_string(),
                );
            }
            let owned_start = inserted_separator
                .filter(|separator| original[..start].ends_with(separator_bytes(*separator)))
                .map_or(start, |separator| start - separator_bytes(separator).len());
            if original.get(end..end + 2) == Some(b"\r\n") {
                end += 2;
            } else if original.get(end) == Some(&b'\n') {
                end += 1;
            }
            let mut output = Vec::with_capacity(original.len() - (end - owned_start));
            output.extend_from_slice(&original[..owned_start]);
            output.extend_from_slice(&original[end..]);
            Ok(Some(output))
        }
        GuidanceKind::CursorRule | GuidanceKind::CopilotInstructions | GuidanceKind::TraeRule => {
            if sha256(original) != managed_hash {
                return Err(
                    "The FastCtx-owned guidance file drifted after Apply; FastCtx will not delete user-changed bytes."
                        .to_string(),
                );
            }
            if original_existed {
                Err(
                    "The guidance receipt says FastCtx did not create this whole-file rule; move it aside manually before Disconnect."
                        .to_string(),
                )
            } else {
                Ok(None)
            }
        }
    }
}

pub(crate) fn guidance_managed_hash(
    target: AgentTarget,
    bytes: &[u8],
) -> Result<Option<String>, String> {
    match target.guidance_kind() {
        GuidanceKind::SharedMarkdown => {
            let source = utf8(bytes)?;
            Ok(section_range(source)?.map(|(start, end)| sha256(&bytes[start..end])))
        }
        GuidanceKind::CursorRule | GuidanceKind::CopilotInstructions | GuidanceKind::TraeRule => {
            Ok(Some(sha256(bytes)))
        }
    }
}

fn apply_shared(original: &[u8], body: &str, owned: bool) -> Result<GuidanceEdit, String> {
    let source = utf8(original)?;
    let block = format!("{BEGIN}\n{body}{END}");
    let managed_hash = sha256(block.as_bytes());
    match section_range(source)? {
        Some(_) if !owned => Err(
            "A FastCtx guidance marker block already exists without an ownership receipt. Review or remove it, then retry Apply."
                .to_string(),
        ),
        Some((start, end)) => {
            let mut bytes = Vec::with_capacity(original.len() + block.len() - (end - start));
            bytes.extend_from_slice(&original[..start]);
            bytes.extend_from_slice(block.as_bytes());
            bytes.extend_from_slice(&original[end..]);
            Ok(GuidanceEdit {
                bytes,
                managed_hash,
                inserted_separator: None,
            })
        }
        None => {
            let mut bytes = original.to_vec();
            let inserted_separator = if bytes.is_empty()
                || bytes.ends_with(b"\r\n\r\n")
                || bytes.ends_with(b"\n\n")
            {
                None
            } else if bytes.ends_with(b"\r\n") {
                bytes.extend_from_slice(b"\r\n");
                Some(InsertedSeparator::CrLf)
            } else if bytes.ends_with(b"\n") {
                bytes.push(b'\n');
                Some(InsertedSeparator::Lf)
            } else {
                bytes.extend_from_slice(b"\n\n");
                Some(InsertedSeparator::LfLf)
            };
            bytes.extend_from_slice(block.as_bytes());
            bytes.push(b'\n');
            Ok(GuidanceEdit {
                bytes,
                managed_hash,
                inserted_separator,
            })
        }
    }
}

fn apply_dedicated(
    target: AgentTarget,
    original: Option<&[u8]>,
    body: &str,
    owned: bool,
) -> Result<GuidanceEdit, String> {
    if original.is_some() && !owned {
        return Err(format!(
            "{} already exists without a FastCtx ownership receipt. Move or rename it, then retry Apply.",
            target.display_name()
        ));
    }
    let text = match target.guidance_kind() {
        GuidanceKind::CursorRule => {
            format!("---\ndescription: FastCtx local tools\nalwaysApply: true\n---\n\n{body}")
        }
        GuidanceKind::CopilotInstructions => format!("---\napplyTo: \"**\"\n---\n\n{body}"),
        GuidanceKind::TraeRule => format!("---\ndescription: FastCtx local tools\n---\n\n{body}"),
        GuidanceKind::SharedMarkdown => unreachable!(),
    };
    Ok(GuidanceEdit {
        managed_hash: sha256(text.as_bytes()),
        bytes: text.into_bytes(),
        inserted_separator: None,
    })
}

pub(crate) fn generated_guidance(_target: AgentTarget, tools: EnabledTools) -> String {
    let names = tools
        .names()
        .into_iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut text = format!(
        "## FastCtx local tools\n\nUse FastCtx for local filesystem and command work. Enabled tools: {names}.\n"
    );
    if tools.contains("inspect_local_file") {
        text.push_str(
            "Each inspect_local_file call accepts one path; issue independent read-only calls in parallel when the host supports it.\n",
        );
    }
    if tools.contains("grep") || tools.contains("glob") {
        text.push_str(
            "Prefer enabled FastCtx search tools over shell equivalents for repository discovery.\n",
        );
    }
    if tools.contains("replace") {
        text.push_str(
            "Use replace for mechanical edits; use the host editing channel for semantic changes.\n",
        );
    }
    if tools.shell_enabled() {
        text.push_str(
            "Write POSIX bash for run. Use run_background for long work, job_output to inspect it, job_list to rediscover it, and job_kill to stop it.\n",
        );
    }
    text.push_str(
        "Successful responses begin with a factual head note whose ranges and totals describe the body that FastCtx emitted.\n",
    );
    text
}

fn section_range(source: &str) -> Result<Option<(usize, usize)>, String> {
    let begins = source.match_indices(BEGIN).collect::<Vec<_>>();
    let ends = source.match_indices(END).collect::<Vec<_>>();
    if begins.is_empty() && ends.is_empty() {
        return Ok(None);
    }
    if begins.len() != 1 || ends.len() != 1 || ends[0].0 < begins[0].0 {
        return Err(
            "Guidance contains duplicate, unmatched, or reversed FastCtx markers. Repair the marker block and retry."
                .to_string(),
        );
    }
    Ok(Some((begins[0].0, ends[0].0 + END.len())))
}

fn utf8(bytes: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(bytes)
        .map_err(|error| format!("Agent guidance is not valid UTF-8 ({error})."))
}

fn separator_bytes(separator: InsertedSeparator) -> &'static [u8] {
    match separator {
        InsertedSeparator::Lf => b"\n",
        InsertedSeparator::CrLf => b"\r\n",
        InsertedSeparator::LfLf => b"\n\n",
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
