//! A connection told before it is made (`world-layers.md`, 接入之前讲清楚做了什么).
//!
//! A login's planners make a change step by step, each step one kind of
//! change to one file. The steps are kept in the order they are made, so
//! that each kind's lines are told apart even where several kinds change the
//! same file; the window before the change shows one card each kind, with
//! the lines it adds and takes out. The fingerprint of the files a change is
//! worked out from refuses the change when they moved on since it was shown:
//! the change made is the one shown.

use std::path::{Path, PathBuf};

use fastctx_types::logins::{Change, ChangeCard, FileDiff, FileHow};
use fastctx_types::run_facts::{DiffLine, DiffLineKind};
use imara_diff::{Algorithm, Diff, InternedInput};
use sha2::{Digest, Sha256};

/// Unchanged lines shown around each change.
const CONTEXT: usize = 2;
/// The most lines one file of one card carries: a guard against a file the
/// user filled, far above anything the World writes.
const MOST_LINES: usize = 2000;

/// One step of a change: what one kind of change does to one file.
#[derive(Clone, Debug)]
struct Step {
    change: Change,
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
}

/// The steps of one change, in the order its planners make them, and the
/// agent's own commands it runs.
#[derive(Clone, Debug, Default)]
pub struct Steps {
    steps: Vec<Step>,
    commands: Vec<(Change, String)>,
}

impl Steps {
    /// Notes that `change` took `path` from `before` to `after` (`None`: no
    /// file). A step that changes nothing is not one.
    pub fn record(&mut self, change: Change, path: impl Into<PathBuf>, before: Option<&[u8]>, after: Option<&[u8]>) {
        if before == after {
            return;
        }
        self.steps.push(Step {
            change,
            path: path.into(),
            before: before.map(<[u8]>::to_vec),
            after: after.map(<[u8]>::to_vec),
        });
    }

    /// Notes that `change` took a file a planner works on in memory from
    /// `bytes` to `next`, where `now` is the file as it stands so far
    /// (`None` while there is no file); a missing file is worked on as
    /// empty, so a step that leaves `bytes` as they were is not one.
    pub fn advance(&mut self, change: Change, path: &str, now: &mut Option<Vec<u8>>, bytes: &[u8], next: &[u8]) {
        if bytes != next {
            self.record(change, path, now.as_deref(), Some(next));
            *now = Some(next.to_vec());
        }
    }

    /// Notes one of the agent's own commands that `change` runs, as run.
    pub fn command(&mut self, change: Change, line: String) {
        self.commands.push((change, line));
    }

    /// `other`'s steps, after these.
    pub fn extend(&mut self, other: Steps) {
        self.steps.extend(other.steps);
        self.commands.extend(other.commands);
    }

    /// One card each kind of change, in the order each was first made. Steps
    /// of one kind that follow each other in a file are told as one.
    pub fn cards(&self) -> Vec<ChangeCard> {
        let mut order: Vec<Change> = Vec::new();
        let mut note = |change: Change| {
            if !order.contains(&change) {
                order.push(change);
            }
        };
        for step in &self.steps {
            note(step.change);
        }
        for (change, _) in &self.commands {
            note(*change);
        }
        order
            .into_iter()
            .map(|change| ChangeCard {
                change,
                files: self.files(change),
                commands: self.commands.iter().filter(|(of, _)| *of == change).map(|(_, line)| line.clone()).collect(),
            })
            .collect()
    }

    fn files(&self, change: Change) -> Vec<FileDiff> {
        // Each file's span of this kind: from the first step's before to the
        // last step's after, while no other kind changed it between.
        let mut spans: Vec<Span> = Vec::new();
        let mut last_kind: Vec<(PathBuf, Change)> = Vec::new();
        for step in &self.steps {
            let previous = last_kind.iter().position(|(path, _)| *path == step.path);
            let joins = step.change == change
                && previous.is_some_and(|at| last_kind[at].1 == change)
                && spans.last().is_some_and(|span| span.path == step.path && span.after == step.before);
            if step.change == change {
                if joins {
                    spans.last_mut().expect("the span joined").after = step.after.clone();
                } else {
                    spans.push(Span {
                        path: step.path.clone(),
                        before: step.before.clone(),
                        after: step.after.clone(),
                    });
                }
            }
            match previous {
                Some(at) => last_kind[at].1 = step.change,
                None => last_kind.push((step.path.clone(), step.change)),
            }
        }
        spans
            .into_iter()
            .filter(|span| span.before != span.after)
            .map(|Span { path, before, after }| {
                let how = match (&before, &after) {
                    (None, _) => FileHow::Created,
                    (_, None) => FileHow::Removed,
                    _ => FileHow::Changed,
                };
                let (lines, more) = diff(before.as_deref(), after.as_deref());
                FileDiff { path, how, lines, more }
            })
            .collect()
    }
}

/// One file as one kind of change took it, from its first step to its last.
struct Span {
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
}

/// The lines `before` and `after` differ in, with `CONTEXT` lines around
/// each change, and how many were left off past `MOST_LINES`.
fn diff(before: Option<&[u8]>, after: Option<&[u8]>) -> (Vec<DiffLine>, Option<u64>) {
    let before = String::from_utf8_lossy(before.unwrap_or_default());
    let after = String::from_utf8_lossy(after.unwrap_or_default());
    let old: Vec<&str> = before.split_inclusive('\n').collect();
    let new: Vec<&str> = after.split_inclusive('\n').collect();
    let mut input = InternedInput::default();
    input.update_before(old.iter().copied());
    input.update_after(new.iter().copied());
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);
    let mut lines = Vec::new();
    let mut more = 0u64;
    let mut row = |kind: DiffLineKind, before: Option<usize>, after: Option<usize>, text: &str| {
        if lines.len() >= MOST_LINES {
            more += 1;
            return;
        }
        let ending = if text.ends_with("\r\n") {
            "\r\n"
        } else if text.ends_with('\n') {
            "\n"
        } else {
            ""
        };
        let text = &text[..text.len() - ending.len()];
        lines.push(DiffLine {
            kind,
            before: before.map(|line| line as u64 + 1),
            after: after.map(|line| line as u64 + 1),
            text: masked(text),
            ending: ending.into(),
        });
    };
    let hunks: Vec<_> = diff.hunks().collect();
    let mut previous_after = 0;
    for (index, hunk) in hunks.iter().enumerate() {
        let (old_start, new_start) = (hunk.before.start as usize, hunk.after.start as usize);
        let context = CONTEXT.min(old_start).min(new_start.saturating_sub(previous_after));
        row(DiffLineKind::Hunk, Some(old_start - context), Some(new_start - context), "");
        for offset in 0..context {
            let (at_old, at_new) = (old_start - context + offset, new_start - context + offset);
            row(DiffLineKind::Context, Some(at_old), Some(at_new), old[at_old]);
        }
        for at in hunk.before.clone() {
            row(DiffLineKind::Removed, Some(at as usize), None, old[at as usize]);
        }
        for at in hunk.after.clone() {
            row(DiffLineKind::Added, None, Some(at as usize), new[at as usize]);
        }
        let (old_end, new_end) = (hunk.before.end as usize, hunk.after.end as usize);
        let context = CONTEXT.min(old.len() - old_end).min(new.len() - new_end);
        let context = hunks.get(index + 1).map_or(context, |next| {
            context.min(next.before.start as usize - old_end).min(next.after.start as usize - new_end)
        });
        for offset in 0..context {
            row(DiffLineKind::Context, Some(old_end + offset), Some(new_end + offset), new[new_end + offset]);
        }
        previous_after = new_end + context;
    }
    (lines, (more > 0).then_some(more))
}

/// `line` with each quoted value of a key that might hold a secret masked:
/// a token, a password, a key. The window shows the lines around a change,
/// which are the user's, and it may be looked at from another device. A
/// value of digits alone is a limit or a count, not a secret, and stays.
fn masked(line: &str) -> String {
    use std::sync::LazyLock;
    const SECRET: &str = "token|secret|password|passwd|api_?key|auth|credential|cookie|private";
    // `key = "value"` anywhere on the line (TOML and its inline tables,
    // environment files), and `"key": "value"` (JSON).
    static ASSIGNED: LazyLock<regex_lite::Regex> = LazyLock::new(|| {
        regex_lite::Regex::new(&format!(r#"(?i)([\w.\-]*(?:{SECRET})[\w.\-]*\s*=\s*)("(?:[^"\\]|\\.)*"|'[^']*')"#))
            .expect("a valid pattern")
    });
    static QUOTED: LazyLock<regex_lite::Regex> = LazyLock::new(|| {
        regex_lite::Regex::new(&format!(r#"(?i)("[^"]*(?:{SECRET})[^"]*"\s*:\s*)("(?:[^"\\]|\\.)*")"#))
            .expect("a valid pattern")
    });
    let mask = |captures: &regex_lite::Captures<'_>| {
        let value = &captures[2];
        let inner = &value[1..value.len() - 1];
        if inner.is_empty() || inner.bytes().all(|byte| byte.is_ascii_digit()) {
            format!("{}{value}", &captures[1])
        } else {
            format!("{}\"•••\"", &captures[1])
        }
    };
    let line = ASSIGNED.replace_all(line, &mask);
    QUOTED.replace_all(&line, &mask).into_owned()
}

/// What a change is worked out from: the bytes of each file it reads, or
/// that there was none, in a fingerprint short enough to send and compare.
pub fn fingerprint(inputs: &[(&Path, Option<&[u8]>)]) -> String {
    let mut hash = Sha256::new();
    for (path, bytes) in inputs {
        let path = path.to_string_lossy();
        hash.update((path.len() as u64).to_le_bytes());
        hash.update(path.as_bytes());
        match bytes {
            Some(bytes) => {
                hash.update([1]);
                hash.update((bytes.len() as u64).to_le_bytes());
                hash.update(bytes);
            }
            None => hash.update([0]),
        }
    }
    hash.finalize()[..16].iter().map(|byte| format!("{byte:02x}")).collect()
}
