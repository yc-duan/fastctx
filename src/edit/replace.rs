//! Two-pass batch replacement with full blast-radius counting and per-file CAS commits.

use super::document::{MAX_REPLACE_RESULT_BYTES, TextDocument};
use super::locks::{FilePathLock, PathIdentity};
use super::{ReplaceRequest, ReplaceService, edit_token_budget, plural};
use crate::budget::{assemble_text, estimate_tokens};
use crate::model::ToolResponse;
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::{Captures, Regex, RegexBuilder};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_CANDIDATES: usize = 10_000;
const MAX_STORED_PREVIEWS: usize = 100_000;

#[derive(Debug)]
struct AnalyzedFile {
    path: String,
    name_identity: PathIdentity,
    identity: PathIdentity,
    revision: String,
    matches: usize,
    previews: Vec<String>,
    previews_truncated: bool,
    used_fallback: bool,
}

#[derive(Debug)]
struct Issue {
    path: String,
    message: String,
}

#[derive(Debug)]
struct ReportGroup {
    lines: Vec<String>,
}

pub(super) fn replace(editor: &ReplaceService, request: ReplaceRequest) -> ToolResponse {
    let budget = match edit_token_budget() {
        Ok(budget) => budget,
        Err(error) => return ToolResponse::error(error),
    };
    if request.path.is_empty() {
        return ToolResponse::error(
            "The path parameter is required. Give the absolute file or directory to edit.",
        );
    }
    let root = match resolve_root(&request.path) {
        Ok(root) => root,
        Err(error) => return ToolResponse::error(error),
    };
    let single_file = root.is_file();
    if single_file && request.fallback_encoding.is_some() {
        return ToolResponse::error(
            "The fallback_encoding parameter only applies to directory targets; use encoding for a single file.",
        );
    }
    if !single_file && request.encoding.is_some() {
        return ToolResponse::error(
            "The encoding parameter only applies to single-file targets; use fallback_encoding for a directory.",
        );
    }
    if let Some(encoding) = request.encoding.as_deref()
        && let Err(rejection) = crate::encoding::canonical_encoding_label(encoding)
    {
        return ToolResponse::error(rejection.message(&crate::paths::display_path(&root)));
    }
    if let Some(encoding) = request.fallback_encoding.as_deref()
        && let Err(rejection) = crate::encoding::canonical_encoding_label(encoding)
    {
        return ToolResponse::error(rejection.message(&crate::paths::display_path(&root)));
    }
    let compiled = match build_regex(&request) {
        Ok(compiled) => compiled,
        Err(error) => return ToolResponse::error(error),
    };
    if compiled.can_match_empty && request.max_replacements.is_none() {
        return ToolResponse::error(
            "This pattern can match empty (zero-width) and would insert at every position. Set max_replacements to cap the blast radius, then retry.",
        );
    }
    if let Err(error) = validate_replacement_references(&compiled.regex, &request.replacement) {
        return ToolResponse::error(error);
    }
    let regex = compiled.regex;
    let glob = match build_glob(request.glob.as_deref()) {
        Ok(glob) => glob,
        Err(error) => return ToolResponse::error(error),
    };
    let candidates = match crate::traversal::collect_project_candidates(&root, glob.as_ref(), None)
    {
        Ok(candidates) => candidates,
        Err(error) => return ToolResponse::error(error),
    };
    if candidates.len() > MAX_CANDIDATES {
        return ToolResponse::error(
            "Too many candidate files: over 10000 matched. Narrow the path or glob.",
        );
    }

    let mut analyzed = Vec::new();
    let mut skipped = Vec::new();
    let mut planning_failures = Vec::new();
    let mut seen_identities = BTreeMap::new();
    let mut total_matches = 0_usize;
    let mut preview_slots = budget
        .saturating_mul(4)
        .saturating_add(32)
        .clamp(1, MAX_STORED_PREVIEWS);
    for candidate in candidates {
        let opened = open_candidate(
            &candidate.display,
            request.encoding.as_deref(),
            request.fallback_encoding.as_deref(),
        );
        let (document, used_fallback) = match opened {
            Ok(opened) => opened,
            Err(error) if is_binary_error(&error) => {
                if single_file {
                    return ToolResponse::error(error);
                }
                skipped.push(Issue {
                    path: candidate.display,
                    message: "binary file".to_string(),
                });
                continue;
            }
            Err(error) if is_skippable_error(&error) => {
                if single_file {
                    return ToolResponse::error(error);
                }
                skipped.push(Issue {
                    path: candidate.display,
                    message: short_issue(&error),
                });
                continue;
            }
            Err(error) => {
                if single_file {
                    return ToolResponse::error(error);
                }
                planning_failures.push(Issue {
                    path: candidate.display,
                    message: error,
                });
                continue;
            }
        };
        let name_identity = match PathIdentity::for_name(document.target_path()) {
            Ok(identity) => identity,
            Err(error) => {
                if single_file {
                    return ToolResponse::error(error);
                }
                planning_failures.push(Issue {
                    path: candidate.display,
                    message: error,
                });
                continue;
            }
        };
        let identity = match PathIdentity::for_existing(document.target_path()) {
            Ok(identity) => identity,
            Err(error) => {
                if single_file {
                    return ToolResponse::error(error);
                }
                planning_failures.push(Issue {
                    path: candidate.display,
                    message: error,
                });
                continue;
            }
        };
        if seen_identities
            .insert(identity.clone(), document.display_path())
            .is_some()
        {
            continue;
        }
        let analysis = analyze_file(&document, &regex, &request.replacement, preview_slots);
        preview_slots = preview_slots.saturating_sub(analysis.previews.len());
        total_matches = total_matches.saturating_add(analysis.matches);
        if analysis.matches == 0 {
            continue;
        }
        if let Err(message) = build_replacement(&document, &regex, &request.replacement) {
            if single_file {
                return ToolResponse::error(message);
            }
            planning_failures.push(Issue {
                path: document.display_path(),
                message,
            });
            continue;
        }
        analyzed.push(AnalyzedFile {
            path: document.display_path(),
            name_identity,
            identity,
            revision: document.revision(),
            matches: analysis.matches,
            previews_truncated: analysis.matches > analysis.previews.len(),
            previews: analysis.previews,
            used_fallback,
        });
    }

    if let Some(maximum) = request.max_replacements
        && total_matches > maximum
    {
        return ToolResponse::error(format!(
            "Refusing to write: {total_matches} matches exceed max_replacements={maximum}. Raise the cap or narrow the pattern; nothing was written."
        ));
    }

    let dry_run = request.dry_run.unwrap_or(false);
    let fallback_label = request
        .fallback_encoding
        .as_deref()
        .and_then(|value| crate::encoding::canonical_encoding_label(value).ok());
    if dry_run {
        return format_dry_run(
            &analyzed,
            &skipped,
            &planning_failures,
            total_matches,
            budget,
            fallback_label,
        );
    }

    let mut successes = Vec::new();
    let mut failures = planning_failures;
    let mut written_replacements = 0_usize;
    let mut ordered = analyzed.iter().enumerate().collect::<Vec<_>>();
    ordered.sort_by(|(_, left), (_, right)| {
        left.identity
            .cmp(&right.identity)
            .then_with(|| left.path.as_bytes().cmp(right.path.as_bytes()))
    });
    for (original_index, file) in ordered {
        let path = Path::new(&file.path);
        let name_process_lock = editor.path_locks.for_identity(&file.name_identity);
        let _name_process_guard = name_process_lock.lock().unwrap();
        let _name_file_guard = match FilePathLock::acquire(&file.name_identity, path) {
            Ok(guard) => guard,
            Err(error) => {
                failures.push(Issue {
                    path: file.path.clone(),
                    message: error,
                });
                continue;
            }
        };
        let target_process_lock = editor.path_locks.for_identity(&file.identity);
        let _target_process_guard = target_process_lock.lock().unwrap();
        let _target_file_guard = match FilePathLock::acquire(&file.identity, path) {
            Ok(guard) => guard,
            Err(error) => {
                failures.push(Issue {
                    path: file.path.clone(),
                    message: error,
                });
                continue;
            }
        };
        let document = match TextDocument::open(
            &file.path,
            if single_file {
                request.encoding.as_deref()
            } else if file.used_fallback {
                request.fallback_encoding.as_deref()
            } else {
                None
            },
        ) {
            Ok(document) => document,
            Err(error) => {
                failures.push(Issue {
                    path: file.path.clone(),
                    message: error,
                });
                continue;
            }
        };
        let current_identity = match PathIdentity::for_existing(document.target_path()) {
            Ok(identity) => identity,
            Err(error) => {
                failures.push(Issue {
                    path: file.path.clone(),
                    message: error,
                });
                continue;
            }
        };
        if current_identity != file.identity {
            failures.push(Issue {
                path: file.path.clone(),
                message: format!(
                    "{} changed on disk during the edit; nothing was written. Re-read it and retry.",
                    file.path
                ),
            });
            continue;
        }
        if document.revision() != file.revision {
            failures.push(Issue {
                path: file.path.clone(),
                message: format!(
                    "{} changed on disk during the edit; nothing was written. Re-read it and retry.",
                    file.path
                ),
            });
            continue;
        }
        let built = match build_replacement(&document, &regex, &request.replacement) {
            Ok(built) => built,
            Err(error) => {
                failures.push(Issue {
                    path: file.path.clone(),
                    message: error,
                });
                continue;
            }
        };
        if built.matches != file.matches {
            failures.push(Issue {
                path: file.path.clone(),
                message: format!(
                    "{} changed on disk during the edit; nothing was written. Re-read it and retry.",
                    file.path
                ),
            });
            continue;
        }
        if built.bytes == document.original_bytes() {
            successes.push((original_index, file.path.clone(), built.matches));
            written_replacements = written_replacements.saturating_add(built.matches);
            continue;
        }
        match document.commit(&built.bytes) {
            Ok(()) => {
                successes.push((original_index, file.path.clone(), built.matches));
                written_replacements = written_replacements.saturating_add(built.matches);
            }
            Err(error) => failures.push(Issue {
                path: file.path.clone(),
                message: error,
            }),
        }
    }

    successes.sort_by_key(|(index, _, _)| *index);
    let successes = successes
        .into_iter()
        .map(|(_, path, matches)| (path, matches))
        .collect::<Vec<_>>();
    failures.sort_by(|left, right| {
        left.path
            .as_bytes()
            .cmp(right.path.as_bytes())
            .then_with(|| left.message.as_bytes().cmp(right.message.as_bytes()))
    });

    format_apply(
        &successes,
        &skipped,
        &failures,
        written_replacements,
        budget,
        &fallback_note(&analyzed, fallback_label),
    )
}

struct FileAnalysis {
    matches: usize,
    previews: Vec<String>,
}

fn analyze_file(
    document: &TextDocument,
    regex: &Regex,
    replacement: &str,
    preview_limit: usize,
) -> FileAnalysis {
    let mut matches = 0_usize;
    let mut previews = Vec::new();
    for captures in regex.captures_iter(document.logical_text()) {
        let matched = captures.get(0).expect("every capture set has group zero");
        let expanded = expand(&captures, replacement);
        if matched.start() == matched.end() && expanded.is_empty() {
            continue;
        }
        matches = matches.saturating_add(1);
        if previews.len() < preview_limit {
            let line = document.logical_text()[..matched.start()]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1;
            previews.push(format!(
                "{line}: {} -> {}",
                preview_text(matched.as_str()),
                preview_text(&expanded)
            ));
        }
    }
    FileAnalysis { matches, previews }
}

struct BuiltReplacement {
    bytes: Vec<u8>,
    matches: usize,
}

fn build_replacement(
    document: &TextDocument,
    regex: &Regex,
    replacement: &str,
) -> Result<BuiltReplacement, String> {
    let mut output = Vec::with_capacity(document.original_bytes().len());
    let mut previous_raw = 0_usize;
    let mut previous_logical = 0_usize;
    let mut matches = 0_usize;
    let mut result_ends_newline = false;
    for captures in regex.captures_iter(document.logical_text()) {
        let matched = captures.get(0).expect("every capture set has group zero");
        let expanded = expand(&captures, replacement);
        if matched.start() == matched.end() && expanded.is_empty() {
            continue;
        }
        let raw_start = document.raw_offset(matched.start())?;
        let raw_end = document.raw_offset(matched.end())?;
        extend_checked(
            &mut output,
            &document.original_bytes()[previous_raw..raw_start],
            &document.display_path(),
        )?;
        let unchanged = &document.logical_text()[previous_logical..matched.start()];
        observe_tail(unchanged, &mut result_ends_newline);
        let encoded = document.encode_for_target(&expanded)?;
        extend_checked(&mut output, &encoded, &document.display_path())?;
        observe_tail(&expanded, &mut result_ends_newline);
        previous_raw = raw_end;
        previous_logical = matched.end();
        matches = matches.saturating_add(1);
    }
    extend_checked(
        &mut output,
        &document.original_bytes()[previous_raw..],
        &document.display_path(),
    )?;
    observe_tail(
        &document.logical_text()[previous_logical..],
        &mut result_ends_newline,
    );

    let newline = document.encode_for_target("\n")?;
    if document.trailing_newline() && !result_ends_newline {
        extend_checked(&mut output, &newline, &document.display_path())?;
    } else if !document.trailing_newline() && result_ends_newline && output.ends_with(&newline) {
        output.truncate(output.len() - newline.len());
    }
    Ok(BuiltReplacement {
        bytes: output,
        matches,
    })
}

fn extend_checked(output: &mut Vec<u8>, bytes: &[u8], path: &str) -> Result<(), String> {
    checked_result_size(output.len(), bytes.len(), path)?;
    output.extend_from_slice(bytes);
    Ok(())
}

fn checked_result_size(current: usize, additional: usize, path: &str) -> Result<usize, String> {
    let projected = current.saturating_add(additional);
    if projected > MAX_REPLACE_RESULT_BYTES {
        return Err(format!(
            "Refusing to write {path}: the result would be {:.1} MiB, over the 256 MiB safety limit. Narrow the pattern.",
            projected as f64 / 1_048_576.0
        ));
    }
    Ok(projected)
}

fn observe_tail(text: &str, ends_newline: &mut bool) {
    if !text.is_empty() {
        *ends_newline = text.ends_with('\n');
    }
}

fn expand(captures: &Captures<'_>, replacement: &str) -> String {
    let mut expanded = String::new();
    captures.expand(replacement, &mut expanded);
    expanded
}

fn open_candidate(
    path: &str,
    explicit: Option<&str>,
    fallback: Option<&str>,
) -> Result<(TextDocument, bool), String> {
    match TextDocument::open(path, explicit) {
        Ok(document) => Ok((document, false)),
        Err(error)
            if explicit.is_none()
                && fallback.is_some()
                && is_encoding_error(&error)
                && !error.contains("byte order mark") =>
        {
            TextDocument::open(path, fallback).map(|document| (document, true))
        }
        Err(error) => Err(error),
    }
}

#[derive(Debug)]
struct CompiledRegex {
    regex: Regex,
    can_match_empty: bool,
}

fn build_regex(request: &ReplaceRequest) -> Result<CompiledRegex, String> {
    if request.pattern.is_empty() {
        return Err(
            "An empty pattern matches at every position and is almost always a mistake. Give a non-empty pattern."
                .to_string(),
        );
    }
    let pattern = if request.literal.unwrap_or(false) {
        regex::escape(&request.pattern)
    } else {
        request.pattern.clone()
    };
    let regex = RegexBuilder::new(&pattern)
        .case_insensitive(request.case_insensitive.unwrap_or(false))
        .dot_matches_new_line(request.dot_all.unwrap_or(false))
        .build()
        .map_err(|error| {
            format!(
                "Invalid regex pattern: {error}\nNote: Rust regex syntax — no lookaround or backreferences; escape literal braces."
            )
        })?;
    let hir = regex_syntax::Parser::new().parse(&pattern).map_err(|error| {
        format!(
            "Invalid regex pattern: {error}\nNote: Rust regex syntax — no lookaround or backreferences; escape literal braces."
        )
    })?;
    Ok(CompiledRegex {
        regex,
        can_match_empty: hir.properties().minimum_len() == Some(0),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReplacementReference<'a> {
    Number(usize),
    Named(&'a str),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReplacementToken<'a> {
    token: &'a str,
    reference: ReplacementReference<'a>,
}

fn validate_replacement_references(regex: &Regex, replacement: &str) -> Result<(), String> {
    let names = regex.capture_names().flatten().collect::<Vec<_>>();
    for token in replacement_tokens(replacement) {
        let defined = match token.reference {
            ReplacementReference::Number(index) => index < regex.captures_len(),
            ReplacementReference::Named(name) => names.contains(&name),
        };
        if !defined {
            return Err(format!(
                "Replacement references an undefined capture group: {}. The pattern defines {}. Fix the replacement; nothing was written.",
                token.token,
                available_groups(regex, &names)
            ));
        }
    }
    Ok(())
}

fn available_groups(regex: &Regex, names: &[&str]) -> String {
    let numbered = regex.captures_len().saturating_sub(1);
    match (numbered, names) {
        (0, []) => "no capture groups".to_string(),
        (1, []) => "group 1".to_string(),
        (count, []) => format!("groups 1-{count}"),
        (0, [name]) => format!("named group: {name}"),
        (0, names) => format!("named groups: {}", names.join(", ")),
        (1, [name]) => format!("group 1; named group: {name}"),
        (count, [name]) => format!("groups 1-{count}; named group: {name}"),
        (1, names) => format!("group 1; named groups: {}", names.join(", ")),
        (count, names) => format!("groups 1-{count}; named groups: {}", names.join(", ")),
    }
}

fn replacement_tokens(replacement: &str) -> Vec<ReplacementToken<'_>> {
    let bytes = replacement.as_bytes();
    let mut tokens = Vec::new();
    let mut cursor = 0_usize;
    while cursor < bytes.len() {
        let Some(relative) = bytes[cursor..].iter().position(|byte| *byte == b'$') else {
            break;
        };
        let start = cursor + relative;
        if bytes.get(start + 1) == Some(&b'$') {
            cursor = start + 2;
            continue;
        }
        let Some(next) = bytes.get(start + 1).copied() else {
            break;
        };
        let (reference_text, end) = if next == b'{' {
            let content_start = start + 2;
            let Some(relative_end) = bytes[content_start..].iter().position(|byte| *byte == b'}')
            else {
                cursor = start + 1;
                continue;
            };
            let content_end = content_start + relative_end;
            (&replacement[content_start..content_end], content_end + 1)
        } else {
            let content_start = start + 1;
            let mut content_end = content_start;
            while bytes
                .get(content_end)
                .is_some_and(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'_'))
            {
                content_end += 1;
            }
            if content_end == content_start {
                cursor = start + 1;
                continue;
            }
            (&replacement[content_start..content_end], content_end)
        };
        let reference = reference_text
            .parse::<usize>()
            .map(ReplacementReference::Number)
            .unwrap_or(ReplacementReference::Named(reference_text));
        tokens.push(ReplacementToken {
            token: &replacement[start..end],
            reference,
        });
        cursor = end;
    }
    tokens
}

fn build_glob(pattern: Option<&str>) -> Result<Option<GlobSet>, String> {
    let Some(pattern) = pattern else {
        return Ok(None);
    };
    let glob = Glob::new(pattern).map_err(|error| {
        format!("Invalid glob pattern: {error}. Use forms like \"*.rs\" or \"**/*.{{ts,tsx}}\".")
    })?;
    let mut builder = GlobSetBuilder::new();
    builder.add(glob);
    builder.build().map(Some).map_err(|error| {
        format!("Invalid glob pattern: {error}. Use forms like \"*.rs\" or \"**/*.{{ts,tsx}}\".")
    })
}

fn resolve_root(input: &str) -> Result<PathBuf, String> {
    let parsed = crate::paths::parse_input_path(input);
    if !parsed.is_absolute() || !parsed.exists() {
        return Err(crate::paths::missing_search_path_message(input));
    }
    fs::metadata(&parsed).map_err(|error| crate::paths::io_error_message(&parsed, &error))?;
    Ok(crate::paths::canonical_existing(&parsed).unwrap_or(parsed))
}

fn format_dry_run(
    analyzed: &[AnalyzedFile],
    skipped: &[Issue],
    failures: &[Issue],
    total_matches: usize,
    budget: usize,
    fallback_label: Option<&str>,
) -> ToolResponse {
    let mut groups = analyzed
        .iter()
        .map(|file| {
            let mut lines = vec![file.path.clone()];
            lines.extend(file.previews.iter().cloned());
            ReportGroup { lines }
        })
        .collect::<Vec<_>>();
    groups.extend(issue_groups(skipped, "skipped"));
    groups.extend(issue_groups(failures, "failed"));
    let matched_files = analyzed.len();
    let mut terminal = if total_matches == 0 {
        "(Complete: dry run — no matches found.)".to_string()
    } else {
        format!(
            "(Complete: dry run — {total_matches} {} in {matched_files} {}; nothing written.)",
            plural(total_matches, "match", "matches"),
            plural(matched_files, "file", "files")
        )
    };
    if !skipped.is_empty() || !failures.is_empty() {
        terminal = append_terminal_clause(
            &terminal,
            &format!(
                "{} {} skipped",
                skipped.len() + failures.len(),
                plural(skipped.len() + failures.len(), "file", "files")
            ),
        );
    }
    render_report(
        &groups,
        &terminal,
        &fallback_note(analyzed, fallback_label),
        budget,
        analyzed.iter().any(|file| file.previews_truncated),
    )
}

fn format_apply(
    successes: &[(String, usize)],
    skipped: &[Issue],
    failures: &[Issue],
    replacements: usize,
    budget: usize,
    extra_notes: &[String],
) -> ToolResponse {
    let mut groups = successes
        .iter()
        .map(|(path, count)| ReportGroup {
            lines: vec![format!(
                "{path}: {count} {}",
                plural(*count, "replacement", "replacements")
            )],
        })
        .collect::<Vec<_>>();
    groups.extend(issue_groups(skipped, "skipped"));
    groups.extend(issue_groups(failures, "failed"));
    let mut terminal = if replacements == 0 && failures.is_empty() {
        "(Complete: no matches found; nothing written.)".to_string()
    } else if failures.is_empty() {
        format!(
            "(Complete: {replacements} {} in {} {}.)",
            plural(replacements, "replacement", "replacements"),
            successes.len(),
            plural(successes.len(), "file", "files")
        )
    } else {
        format!(
            "(Partial: {replacements} {} written in {} {}; {} {} failed — see the report above.)",
            plural(replacements, "replacement", "replacements"),
            successes.len(),
            plural(successes.len(), "file", "files"),
            failures.len(),
            plural(failures.len(), "file", "files")
        )
    };
    if !skipped.is_empty() {
        terminal = append_terminal_clause(
            &terminal,
            &format!(
                "{} {} skipped",
                skipped.len(),
                plural(skipped.len(), "file", "files")
            ),
        );
    }
    render_report(&groups, &terminal, extra_notes, budget, false)
}

fn render_report(
    groups: &[ReportGroup],
    terminal: &str,
    extra_notes: &[String],
    budget: usize,
    force_truncated: bool,
) -> ToolResponse {
    let all_lines = groups
        .iter()
        .flat_map(|group| group.lines.iter().cloned())
        .collect::<Vec<_>>();
    let mut notes = extra_notes.to_vec();
    notes.push(terminal.to_string());
    let full = assemble_text(&all_lines, &notes);
    if !force_truncated && estimate_tokens(&full) <= budget {
        return ToolResponse::text(full);
    }
    let truncated_terminal = append_terminal_clause(terminal, "list truncated, see the note above");
    let mut shown_lines = Vec::new();
    let mut shown_files = 0_usize;
    for group in groups {
        let start_len = shown_lines.len();
        for line in &group.lines {
            shown_lines.push(line.clone());
            let mut trial_notes = vec![format!(
                "(Note: showing {} of {} files; totals below cover all files.)",
                shown_files + 1,
                groups.len()
            )];
            trial_notes.extend(extra_notes.iter().cloned());
            trial_notes.push(truncated_terminal.clone());
            if estimate_tokens(&assemble_text(&shown_lines, &trial_notes)) > budget {
                shown_lines.pop();
                break;
            }
        }
        if shown_lines.len() > start_len {
            shown_files += 1;
        }
        let mut trial_notes = vec![format!(
            "(Note: showing {shown_files} of {} files; totals below cover all files.)",
            groups.len()
        )];
        trial_notes.extend(extra_notes.iter().cloned());
        trial_notes.push(truncated_terminal.clone());
        if estimate_tokens(&assemble_text(&shown_lines, &trial_notes)) >= budget {
            break;
        }
    }
    let mut truncated_notes = vec![format!(
        "(Note: showing {shown_files} of {} files; totals below cover all files.)",
        groups.len()
    )];
    truncated_notes.extend(extra_notes.iter().cloned());
    truncated_notes.push(truncated_terminal);
    let output = assemble_text(&shown_lines, &truncated_notes);
    if estimate_tokens(&output) <= budget {
        ToolResponse::text(output)
    } else {
        ToolResponse::error(format!(
            "FASTCTX_TOKEN_BUDGET={budget} is too small to return the required status note. Increase it and retry."
        ))
    }
}

fn issue_groups(issues: &[Issue], label: &str) -> Vec<ReportGroup> {
    issues
        .iter()
        .map(|issue| ReportGroup {
            lines: vec![format!("{} — {label}: {}", issue.path, issue.message)],
        })
        .collect()
}

fn fallback_note(analyzed: &[AnalyzedFile], encoding: Option<&str>) -> Vec<String> {
    let count = analyzed.iter().filter(|file| file.used_fallback).count();
    if count == 0 {
        Vec::new()
    } else {
        let encoding = encoding.unwrap_or("the requested fallback");
        vec![format!(
            "(Note: {count} {} decoded using fallback encoding {encoding}.)",
            plural(count, "file", "files"),
        )]
    }
}

fn append_terminal_clause(terminal: &str, clause: &str) -> String {
    let stem = terminal
        .strip_suffix(".)")
        .expect("replace terminal notes always end with .)");
    format!("{stem}; {clause}.)")
}

fn preview_text(text: &str) -> String {
    let escaped = text
        .replace("\r\n", "\\n")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    let total = escaped.chars().count();
    let shown = escaped.chars().take(160).collect::<String>();
    if total > 160 {
        format!("{shown}…")
    } else {
        shown
    }
}

fn is_binary_error(error: &str) -> bool {
    error.starts_with("Cannot read binary file as text:")
}

fn is_encoding_error(error: &str) -> bool {
    error.starts_with("Cannot determine the text encoding") || error.starts_with("Cannot decode ")
}

fn is_skippable_error(error: &str) -> bool {
    is_encoding_error(error) || error.starts_with("File too large for line edits:")
}

fn short_issue(error: &str) -> String {
    if error.contains("mixed or inconsistent encodings") {
        "mixed or inconsistent encodings".to_string()
    } else if error.starts_with("Cannot determine the text encoding") {
        "ambiguous encoding".to_string()
    } else if error.starts_with("Cannot decode ") {
        "undecodable".to_string()
    } else {
        error.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{analyze_file, build_regex, preview_text, validate_replacement_references};
    use crate::edit::{ReplaceRequest, document::TextDocument};

    fn request(pattern: &str, replacement: &str) -> ReplaceRequest {
        ReplaceRequest {
            pattern: pattern.to_string(),
            replacement: replacement.to_string(),
            path: "/tmp".to_string(),
            glob: None,
            literal: None,
            case_insensitive: None,
            dot_all: None,
            max_replacements: None,
            dry_run: None,
            encoding: None,
            fallback_encoding: None,
        }
    }

    #[test]
    fn captures_dollars_and_pattern_width_guards_follow_regex_semantics() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("replace.txt");
        std::fs::write(&path, b"ab ab").unwrap();
        let document = TextDocument::open(path.to_str().unwrap(), None).unwrap();
        let compiled = build_regex(&request("(a)(b)", "$2$1$$")).unwrap();
        let analysis = analyze_file(&document, &compiled.regex, "$2$1$$", usize::MAX);
        assert_eq!(analysis.matches, 2);
        assert!(!compiled.can_match_empty);

        assert_eq!(
            build_regex(&request("", "")).unwrap_err(),
            "An empty pattern matches at every position and is almost always a mistake. Give a non-empty pattern."
        );
        assert!(build_regex(&request("x*", "y")).unwrap().can_match_empty);
        assert!(build_regex(&request(r"\b", "y")).unwrap().can_match_empty);
    }

    #[test]
    fn replacement_references_are_validated_with_the_engine_token_grammar() {
        let compiled = build_regex(&request("(?P<name>a)(b)?", "")).unwrap();
        for replacement in ["$0", "$1", "${1}", "$2", "$name", "${name}", "$$", "$"] {
            validate_replacement_references(&compiled.regex, replacement).unwrap();
        }
        for (replacement, token) in [
            ("$3", "$3"),
            ("${missing}", "${missing}"),
            ("$1a", "$1a"),
            ("$nameX", "$nameX"),
        ] {
            assert_eq!(
                validate_replacement_references(&compiled.regex, replacement).unwrap_err(),
                format!(
                    "Replacement references an undefined capture group: {token}. The pattern defines groups 1-2; named group: name. Fix the replacement; nothing was written."
                )
            );
        }
        validate_replacement_references(&compiled.regex, "${1}a ${name}X").unwrap();
    }

    #[test]
    fn preview_windows_are_single_line_and_character_bounded() {
        assert_eq!(preview_text("a\r\nb"), "a\\nb");
        assert_eq!(preview_text(&"界".repeat(161)).chars().count(), 161);
    }

    #[test]
    fn replacement_size_guard_accepts_the_exact_limit_and_rejects_one_byte_more() {
        assert_eq!(
            super::checked_result_size(super::MAX_REPLACE_RESULT_BYTES - 1, 1, "target"),
            Ok(super::MAX_REPLACE_RESULT_BYTES)
        );
        assert_eq!(
            super::checked_result_size(super::MAX_REPLACE_RESULT_BYTES, 1, "target").unwrap_err(),
            "Refusing to write target: the result would be 256.0 MiB, over the 256 MiB safety limit. Narrow the pattern."
        );
    }
}
