//! Shared wording for whatever a tool could not reach, read, or search.
//!
//! Two kinds of gap reach a response and they mean different things to the
//! caller. A skipped *file* was found and then could not be used, so the count
//! is exact and the next move is usually a decoding or size decision. An
//! unreachable *path* was never entered, so it hides an unknown number of files
//! and the result set has a hole in it — "not found" stops meaning "not there".
//! Every tool states both through this module so one reading works everywhere.

/// One line of skip detail: the path, and why it did not contribute.
pub(crate) fn detail_line(path: &str, reason: &str) -> String {
    format!("{path} — {reason}")
}

/// What a response has to disclose about the gaps in its own coverage.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SkipTally {
    /// Files reached but not used, e.g. undecodable or changed mid-search.
    pub(crate) files: usize,
    /// Paths never entered, each hiding an unknown amount of the tree.
    pub(crate) unreachable: usize,
    /// Detail lines available to show, across both kinds.
    pub(crate) listed: usize,
}

impl SkipTally {
    pub(crate) fn is_empty(&self) -> bool {
        self.files == 0 && self.unreachable == 0
    }

    /// The concise fact inserted into a response head note.
    ///
    /// `shown` is how many detail lines survived the budget; a value below
    /// `listed` adds the hint that narrowing the request reveals the rest.
    pub(crate) fn fact(&self, shown: usize) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut clause = String::new();
        if self.files > 0 {
            clause.push_str(&format!("{} skipped", counted(self.files, "file", "files")));
        }
        if self.unreachable > 0 {
            if !clause.is_empty() {
                clause.push_str(", ");
            }
            // "unreachable" carries the consequence on its own: a path the walk
            // never entered is one these results cannot speak for. Spelling that
            // out again collides with the truncation hint that may follow.
            clause.push_str(&format!(
                "{} unreachable",
                counted(self.unreachable, "path", "paths")
            ));
        }
        if shown < self.listed {
            clause.push_str(&format!(
                ", showing {shown} — narrow path/glob to inspect the rest"
            ));
        }
        Some(clause)
    }
}

fn counted(count: usize, singular: &str, plural: &str) -> String {
    let noun = if count == 1 { singular } else { plural };
    format!("{count} {noun}")
}
