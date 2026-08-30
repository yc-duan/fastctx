//! Two-pass batch replacement with full blast-radius counting and per-file CAS commits.

use super::document::TextDocument;
use super::locks::{FilePathLock, PathIdentity};
use super::{ReplaceRequest, ReplaceService, edit_token_budget, plural};
use crate::budget::{GLOBAL_TOKEN_BUDGET_ENV, estimate_tokens, tool_token_budget_for_required};
use crate::glob_filter::{GlobPatterns, PathGlobFilter};
use crate::head_note::{HeadMetric, HeadNote};
use crate::model::ToolResponse;
use regex::{Captures, Regex, RegexBuilder};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The paths a walk never entered: those worth listing, and how many there were.
#[derive(Clone, Copy)]
struct Unreachable<'a> {
    issues: &'a [Issue],
    total: usize,
}

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

pub(super) fn replace(
    editor: &ReplaceService,
    request: ReplaceRequest,
    max_file_size_mib: u64,
) -> ToolResponse {
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
    let glob = match build_glob(request.glob.as_ref()) {
        Ok(glob) => glob,
        Err(error) => return ToolResponse::error(error),
    };
    let collected = match crate::traversal::collect_project_candidates(&root, glob.as_ref(), None) {
        Ok(collected) => collected,
        Err(error) => return ToolResponse::error(error),
    };
    // Paths the walk never entered hide an unknown number of files. For a tool
    // that writes, that is a coverage hole, not a footnote.
    let unreachable_issues = collected
        .skipped
        .listed()
        .map(|path| Issue {
            path: path.display.to_string(),
            message: path.reason.to_string(),
        })
        .collect::<Vec<_>>();
    let unreachable = Unreachable {
        issues: &unreachable_issues,
        total: collected.skipped.total(),
    };
    let candidates = collected.items;
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
            max_file_size_mib,
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
        if let Err(message) =
            validate_replacement(&document, &regex, &request.replacement, max_file_size_mib)
        {
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
            unreachable,
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
            max_file_size_mib,
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
        let built =
            match build_replacement(&document, &regex, &request.replacement, max_file_size_mib) {
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
        unreachable,
        written_replacements,
        budget,
        &fallback_facts(&analyzed, fallback_label),
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

struct ReplacementOutput {
    bytes: Option<Vec<u8>>,
    len: usize,
}

impl ReplacementOutput {
    fn new(capacity: usize, materialize: bool) -> Self {
        Self {
            bytes: materialize.then(|| Vec::with_capacity(capacity)),
            len: 0,
        }
    }

    fn extend(&mut self, bytes: &[u8], path: &str, max_result_size_mib: u64) -> Result<(), String> {
        self.len = checked_result_size(self.len, bytes.len(), path, max_result_size_mib)?;
        if let Some(output) = &mut self.bytes {
            output.extend_from_slice(bytes);
        }
        Ok(())
    }

    fn remove_suffix(&mut self, suffix: &[u8]) {
        let suffix_is_present = self
            .bytes
            .as_ref()
            .is_none_or(|output| output.ends_with(suffix));
        if suffix_is_present && self.len >= suffix.len() {
            self.len -= suffix.len();
            if let Some(output) = &mut self.bytes {
                output.truncate(self.len);
            }
        }
    }
}

fn validate_replacement(
    document: &TextDocument,
    regex: &Regex,
    replacement: &str,
    max_result_size_mib: u64,
) -> Result<(), String> {
    process_replacement(document, regex, replacement, max_result_size_mib, false).map(drop)
}

fn build_replacement(
    document: &TextDocument,
    regex: &Regex,
    replacement: &str,
    max_result_size_mib: u64,
) -> Result<BuiltReplacement, String> {
    let (output, matches) =
        process_replacement(document, regex, replacement, max_result_size_mib, true)?;
    Ok(BuiltReplacement {
        bytes: output
            .bytes
            .expect("materialized replacement always has an output buffer"),
        matches,
    })
}

fn process_replacement(
    document: &TextDocument,
    regex: &Regex,
    replacement: &str,
    max_result_size_mib: u64,
    materialize: bool,
) -> Result<(ReplacementOutput, usize), String> {
    let mut output = ReplacementOutput::new(document.original_bytes().len(), materialize);
    let mut previous_raw = 0_usize;
    let mut previous_logical = 0_usize;
    let mut matches = 0_usize;
    let mut result_ends_newline = false;
    let mut raw_cursor = document.raw_offset_cursor()?;
    for captures in regex.captures_iter(document.logical_text()) {
        let matched = captures.get(0).expect("every capture set has group zero");
        let expanded = expand(&captures, replacement);
        if matched.start() == matched.end() && expanded.is_empty() {
            continue;
        }
        let raw_start = raw_cursor.advance_to(matched.start())?;
        let raw_end = raw_cursor.advance_to(matched.end())?;
        output.extend(
            &document.original_bytes()[previous_raw..raw_start],
            &document.display_path(),
            max_result_size_mib,
        )?;
        let unchanged = &document.logical_text()[previous_logical..matched.start()];
        observe_tail(unchanged, &mut result_ends_newline);
        let encoded = document.encode_for_target(&expanded)?;
        output.extend(&encoded, &document.display_path(), max_result_size_mib)?;
        observe_tail(&expanded, &mut result_ends_newline);
        previous_raw = raw_end;
        previous_logical = matched.end();
        matches = matches.saturating_add(1);
    }
    output.extend(
        &document.original_bytes()[previous_raw..],
        &document.display_path(),
        max_result_size_mib,
    )?;
    observe_tail(
        &document.logical_text()[previous_logical..],
        &mut result_ends_newline,
    );

    let newline = document.encode_for_target("\n")?;
    if document.trailing_newline() && !result_ends_newline {
        output.extend(&newline, &document.display_path(), max_result_size_mib)?;
    } else if !document.trailing_newline() && result_ends_newline {
        // With no original trailing newline, only encoded replacement text can introduce this
        // final boundary, so the validation-only pass can prove the same suffix without bytes.
        output.remove_suffix(&newline);
    }
    Ok((output, matches))
}

fn checked_result_size(
    current: usize,
    additional: usize,
    path: &str,
    max_result_size_mib: u64,
) -> Result<usize, String> {
    let projected = current.saturating_add(additional);
    let maximum_bytes = max_result_size_mib.saturating_mul(1024 * 1024);
    if u64::try_from(projected).unwrap_or(u64::MAX) > maximum_bytes {
        return Err(format!(
            "Refusing to write {path}: the result would be {:.1} MiB, over the {max_result_size_mib} MiB safety limit. Narrow the pattern.",
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
    max_file_size_mib: u64,
) -> Result<(TextDocument, bool), String> {
    match TextDocument::open(path, explicit, max_file_size_mib) {
        Ok(document) => Ok((document, false)),
        Err(error)
            if explicit.is_none()
                && fallback.is_some()
                && is_encoding_error(&error)
                && !error.contains("byte order mark") =>
        {
            TextDocument::open(path, fallback, max_file_size_mib).map(|document| (document, true))
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

fn build_glob(patterns: Option<&GlobPatterns>) -> Result<Option<PathGlobFilter>, String> {
    let Some(patterns) = patterns else {
        return Ok(None);
    };
    PathGlobFilter::compile(patterns, false)
        .map(Some)
        .map_err(|error| {
            format!(
                "Invalid glob pattern: {error}. Use forms like \"*.rs\" or \"**/*.{{ts,tsx}}\"."
            )
        })
}

fn resolve_root(input: &str) -> Result<PathBuf, String> {
    let parsed = crate::paths::parse_local_path_input(input)?;
    let input_display = crate::paths::display_path(&parsed);
    if !parsed.is_absolute() || !parsed.exists() {
        return Err(crate::paths::missing_search_path_message(&input_display));
    }
    fs::metadata(&parsed).map_err(|error| crate::paths::io_error_message(&parsed, &error))?;
    crate::paths::canonical_existing(&parsed)
        .map_err(|error| crate::paths::io_error_message(&parsed, &error))
}

fn format_dry_run(
    analyzed: &[AnalyzedFile],
    skipped: &[Issue],
    failures: &[Issue],
    unreachable: Unreachable<'_>,
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
    groups.extend(issue_groups(unreachable.issues, "unreachable"));
    let matched_files = analyzed.len();
    let mut note = HeadNote::new(
        "replace dry run",
        HeadMetric::count_in_files(total_matches, "match", "matches", matched_files),
    )
    .fact("nothing written");
    if !skipped.is_empty() {
        note = note.fact(format!(
            "{} {} skipped",
            skipped.len(),
            plural(skipped.len(), "file", "files")
        ));
    }
    if !failures.is_empty() {
        note = note.fact(format!(
            "{} {} failed",
            failures.len(),
            plural(failures.len(), "file", "files")
        ));
    }
    if unreachable.total > 0 {
        note = note.fact(format!(
            "{} {} unreachable",
            unreachable.total,
            plural(unreachable.total, "path", "paths")
        ));
    }
    render_report(
        &groups,
        note,
        &fallback_facts(analyzed, fallback_label),
        budget,
        analyzed.iter().any(|file| file.previews_truncated),
    )
}

fn format_apply(
    successes: &[(String, usize)],
    skipped: &[Issue],
    failures: &[Issue],
    unreachable: Unreachable<'_>,
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
    groups.extend(issue_groups(unreachable.issues, "unreachable"));
    let mut note = HeadNote::new(
        "replace",
        HeadMetric::count_in_files(replacements, "replacement", "replacements", successes.len()),
    );
    if replacements == 0 && failures.is_empty() {
        note = note.fact("nothing written");
    }
    if !skipped.is_empty() {
        note = note.fact(format!(
            "{} {} skipped",
            skipped.len(),
            plural(skipped.len(), "file", "files")
        ));
    }
    if !failures.is_empty() {
        note = note.fact(format!(
            "{} {} failed — see report",
            failures.len(),
            plural(failures.len(), "file", "files")
        ));
    }
    if unreachable.total > 0 {
        note = note.fact(format!(
            "{} {} unreachable",
            unreachable.total,
            plural(unreachable.total, "path", "paths")
        ));
    }
    render_report(&groups, note, extra_notes, budget, false)
}

fn render_report(
    groups: &[ReportGroup],
    note: HeadNote,
    extra_facts: &[String],
    budget: usize,
    force_truncated: bool,
) -> ToolResponse {
    let all_lines = groups
        .iter()
        .flat_map(|group| group.lines.iter().cloned())
        .collect::<Vec<_>>();
    let mut base = note;
    for fact in extra_facts {
        base = base.fact(fact);
    }
    if force_truncated {
        base = base.fact("preview details were capped before rendering");
    }
    let full = base.render_with_body(&all_lines.join("\n"));
    if !force_truncated && estimate_tokens(&full) <= budget {
        return ToolResponse::text(full);
    }
    let total_lines = all_lines.len();
    let mut low = 0_usize;
    let mut high = total_lines;
    let mut best = None;
    while low <= high {
        let shown = low + (high - low) / 2;
        let candidate_note = base
            .clone()
            .fact(format!("report shows {shown} of {total_lines} lines"));
        let candidate = candidate_note.render_with_body(&all_lines[..shown].join("\n"));
        if estimate_tokens(&candidate) <= budget {
            best = Some(candidate);
            low = shown.saturating_add(1);
        } else if shown == 0 {
            break;
        } else {
            high = shown - 1;
        }
    }
    let output = best.unwrap_or_else(|| {
        base.clone()
            .fact(format!("report shows 0 of {total_lines} lines"))
            .render()
    });
    if estimate_tokens(&output) <= budget {
        ToolResponse::text(output)
    } else {
        let required = estimate_tokens(&output);
        if let Ok(expanded) = tool_token_budget_for_required(GLOBAL_TOKEN_BUDGET_ENV, required)
            && expanded.value > budget
        {
            return render_report(groups, base, &[], expanded.value, false);
        }
        ToolResponse::error(format!(
            "FASTCTX_TOKEN_BUDGET={budget} is too small to return the replace head note. That budget is fixed for this session; retrying cannot raise it."
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

fn fallback_facts(analyzed: &[AnalyzedFile], encoding: Option<&str>) -> Vec<String> {
    let count = analyzed.iter().filter(|file| file.used_fallback).count();
    if count == 0 {
        Vec::new()
    } else {
        let encoding = encoding.unwrap_or("the requested fallback");
        vec![format!(
            "{count} {} decoded using fallback encoding {encoding}",
            plural(count, "file", "files"),
        )]
    }
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
