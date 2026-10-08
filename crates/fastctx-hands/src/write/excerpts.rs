use crate::view::visible;
use std::fmt::Write as _;

/// The changed lines with two neighbours on each side, numbered as
/// `fastctx view` numbers them, `number<TAB>line`, with control characters
/// shown the same way.
pub(super) fn excerpts(text: &str, changes: &[std::ops::Range<usize>]) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').map(crate::text::line_content).collect();
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
    let mut windows: Vec<std::ops::Range<usize>> = Vec::new();
    for changed in changes {
        let start = starts.partition_point(|offset| *offset <= changed.start) - 1;
        let end = starts.partition_point(|offset| *offset <= changed.end) - 1;
        // Two neighboring lines on each side provide the requested edit excerpt.
        let window = start.saturating_sub(2)..(end + 3).min(lines.len());
        if let Some(previous) = windows.last_mut()
            && window.start <= previous.end
        {
            previous.end = previous.end.max(window.end);
        } else {
            windows.push(window);
        }
    }
    let mut result = String::new();
    for (index, window) in windows.iter().enumerate() {
        if index > 0 {
            result.push_str("\n...");
        }
        for line in window.clone() {
            let _ = write!(result, "\n{}\t{}", line + 1, visible(lines[line]));
        }
    }
    if lines.is_empty() {
        result.push_str("\n(empty file)");
    }
    result
}
