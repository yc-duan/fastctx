// Derived from OpenAI Codex (Apache-2.0), copyright 2025 OpenAI.
// Modified for FastCtx: module wiring and synchronous text application.
// Upstream revision: 7431f10d0d4b4ecf5df08a853b41859013b17e45.

use super::{parser::UpdateFileChunk, seek_sequence};

struct Replacement {
    start: usize,
    old_len: usize,
    new_lines: Vec<String>,
    new_line_sources: Vec<Option<usize>>,
}

pub(super) fn apply(text: &str, path: &str, chunks: &[UpdateFileChunk], eol: &str) -> Result<String, String> {
    let mut lines = Vec::new();
    let mut endings = Vec::new();
    for line in text.split_inclusive('\n') {
        lines.push(crate::text::line_content(line).to_string());
        endings.push(
            if line.ends_with("\r\n") {
                "\r\n"
            } else if line.ends_with('\n') {
                "\n"
            } else {
                eol
            }
            .into(),
        );
    }
    let replacements = compute_replacements(&lines, path, chunks)?;
    let (lines, endings) = apply_replacements(lines, endings, &replacements, eol);
    let mut result = String::new();
    for (index, line) in lines.iter().enumerate() {
        result.push_str(line);
        // Codex treats a final empty line as the newline sentinel when joining.
        if index + 1 < lines.len() || !line.is_empty() {
            result.push_str(&endings[index]);
        }
    }
    Ok(result)
}

fn compute_replacements(
    original_lines: &[String],
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<Vec<Replacement>, String> {
    let mut replacements = Vec::new();
    let mut line_index: usize = 0;

    for chunk in chunks {
        // If a chunk has a `change_context`, we use seek_sequence to find it, then
        // adjust our `line_index` to continue from there.
        if let Some(ctx_line) = &chunk.change_context {
            if let Some(idx) = seek_sequence::seek_sequence(
                original_lines,
                std::slice::from_ref(ctx_line),
                line_index,
                /*eof*/ false,
            ) {
                line_index = idx + 1;
            } else {
                return Err(format!("Failed to find context '{ctx_line}' in {path}"));
            }
        }

        if chunk.old_lines.is_empty() {
            // Pure addition (no old lines). We'll add them at the end or just
            // before the final empty line if one exists.
            let insertion_idx = if original_lines.last().is_some_and(String::is_empty) {
                original_lines.len() - 1
            } else {
                original_lines.len()
            };
            replacements.push(Replacement {
                start: insertion_idx,
                old_len: 0,
                new_lines: chunk.new_lines.clone(),
                new_line_sources: chunk.new_line_sources.clone(),
            });
            continue;
        }

        // Otherwise, try to match the existing lines in the file with the old lines
        // from the chunk. If found, schedule that region for replacement.
        // Attempt to locate the `old_lines` verbatim within the file.  In many
        // real‑world diffs the last element of `old_lines` is an *empty* string
        // representing the terminating newline of the region being replaced.
        // This sentinel is not present in `original_lines` because we strip the
        // trailing empty slice emitted by `split('\n')`.  If a direct search
        // fails and the pattern ends with an empty string, retry without that
        // final element so that modifications touching the end‑of‑file can be
        // located reliably.

        let mut pattern: &[String] = &chunk.old_lines;
        let mut found = seek_sequence::seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);

        let mut new_slice: &[String] = &chunk.new_lines;

        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            // Retry without the trailing empty line which represents the final
            // newline in the file.
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }

            found = seek_sequence::seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);
        }

        if let Some(start_idx) = found {
            replacements.push(Replacement {
                start: start_idx,
                old_len: pattern.len(),
                new_lines: new_slice.to_vec(),
                new_line_sources: chunk.new_line_sources[..new_slice.len()].to_vec(),
            });
            line_index = start_idx + pattern.len();
        } else {
            return Err(format!("Failed to find expected lines in {}:\n{}", path, chunk.old_lines.join("\n"),));
        }
    }

    replacements.sort_by_key(|replacement| replacement.start);

    Ok(replacements)
}

/// Apply replacements and retain the original endings of unchanged context lines.
fn apply_replacements(
    mut lines: Vec<String>,
    mut endings: Vec<String>,
    replacements: &[Replacement],
    eol: &str,
) -> (Vec<String>, Vec<String>) {
    let original_endings = endings.clone();
    // We must apply replacements in descending order so that earlier replacements
    // don't shift the positions of later ones.
    for replacement in replacements.iter().rev() {
        let start_idx = replacement.start;
        let old_len = replacement.old_len;
        let new_segment = &replacement.new_lines;
        let new_endings: Vec<String> = replacement
            .new_line_sources
            .iter()
            .map(|source| {
                source
                    .and_then(|offset| original_endings.get(start_idx + offset))
                    .map_or_else(|| eol.to_string(), Clone::clone)
            })
            .collect();

        // Remove old lines.
        for _ in 0..old_len {
            if start_idx < lines.len() {
                lines.remove(start_idx);
                endings.remove(start_idx);
            }
        }

        // Insert new lines.
        for (offset, new_line) in new_segment.iter().enumerate() {
            lines.insert(start_idx + offset, new_line.clone());
            endings.insert(start_idx + offset, new_endings[offset].clone());
        }
    }

    (lines, endings)
}
