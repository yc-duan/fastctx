//! Compute line facts while the writer still owns both revisions.

use crate::encoding::file::{Plan, Source};
use fastctx_types::MachineName;
use fastctx_types::run_facts::{DiffLine, DiffLineKind, DiffRef, INLINE_DIFF, WriteFact};
use fastctx_types::write::ChangeKind;
use imara_diff::{Algorithm, Diff, InternedInput};
use std::borrow::Cow;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub(super) struct Capture {
    pub directory: PathBuf,
    pub machine: MachineName,
    pub operation: String,
}

impl Capture {
    pub fn new(directory: PathBuf, machine: MachineName) -> io::Result<Self> {
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let operation = random.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(Self { directory, machine, operation })
    }

    pub fn prepare(
        &self,
        path: &Path,
        before: Option<&[u8]>,
        after: Option<&[u8]>,
        before_encoding: Option<&str>,
        after_encoding: Option<&str>,
        index: usize,
    ) -> Result<Box<WriteFact>, String> {
        let id = format!("{}-{index}", self.operation);
        let kind = if before.is_none() {
            ChangeKind::Added
        } else if after.is_none() {
            ChangeKind::Deleted
        } else {
            ChangeKind::Modified
        };
        let (before_text, after_text) = match (decode(before, before_encoding), decode(after, after_encoding)) {
            (Ok(before), Ok(after)) => (before, after),
            (before, after) => {
                let reason = before
                    .err()
                    .map(|reason| format!("before: {reason}"))
                    .into_iter()
                    .chain(after.err().map(|reason| format!("after: {reason}")))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Ok(Box::new(WriteFact {
                    turn: None, note: None,
                    id,
                    machine: self.machine.clone(),
                    path: path.to_string_lossy().into_owned(),
                    kind,
                    added: None,
                    removed: None,
                    diff: None,
                    unavailable: Some(reason),
                }));
            }
        };
        let before_lines: Vec<_> = before_text.split_inclusive('\n').collect();
        let after_lines: Vec<_> = after_text.split_inclusive('\n').collect();
        let mut input = InternedInput::default();
        input.update_before(before_lines.iter().copied());
        input.update_after(after_lines.iter().copied());
        let mut diff = Diff::compute(Algorithm::Histogram, &input);
        diff.postprocess_lines(&input);
        std::fs::create_dir_all(&self.directory).map_err(|error| error.to_string())?;
        let artifact = self.directory.join(format!("{id}.jsonl"));
        let mut output = io::BufWriter::new(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&artifact)
                .map_err(|error| error.to_string())?,
        );
        // The rows, as long as they are few enough to go with the
        // conversation.
        let mut written: u64 = 0;
        let mut kept = Some(Vec::new());
        let mut row = |kind, before, after, text: &str| -> io::Result<()> {
            let ending = if text.ends_with("\r\n") {
                "\r\n"
            } else if text.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            let text = &text[..text.len() - ending.len()];
            let line = DiffLine { kind, before, after, text: crate::text::visible(text), ending: ending.into() };
            let bytes = serde_json::to_vec(&line)?;
            output.write_all(&bytes)?;
            output.write_all(b"\n")?;
            written += bytes.len() as u64 + 1;
            if written > INLINE_DIFF {
                kept = None;
            } else if let Some(kept) = &mut kept {
                kept.push(line);
            }
            Ok(())
        };
        let mut previous_after = 0;
        let hunks: Vec<_> = diff.hunks().collect();
        for (index, hunk) in hunks.iter().enumerate() {
            let (old_start, new_start) = (hunk.before.start as usize, hunk.after.start as usize);
            let context = 2.min(old_start).min(new_start.saturating_sub(previous_after));
            row(DiffLineKind::Hunk, Some((old_start - context + 1) as u64), Some((new_start - context + 1) as u64), "")
                .map_err(|error| error.to_string())?;
            for offset in 0..context {
                row(
                    DiffLineKind::Context,
                    Some((old_start - context + offset + 1) as u64),
                    Some((new_start - context + offset + 1) as u64),
                    before_lines[old_start - context + offset],
                )
                .map_err(|error| error.to_string())?;
            }
            for index in hunk.before.clone() {
                row(DiffLineKind::Removed, Some(u64::from(index) + 1), None, before_lines[index as usize])
                    .map_err(|error| error.to_string())?;
            }
            for index in hunk.after.clone() {
                row(DiffLineKind::Added, None, Some(u64::from(index) + 1), after_lines[index as usize])
                    .map_err(|error| error.to_string())?;
            }
            let (old_end, new_end) = (hunk.before.end as usize, hunk.after.end as usize);
            let context = 2.min(before_lines.len() - old_end).min(after_lines.len() - new_end);
            let context = hunks.get(index + 1).map_or(context, |next| {
                context.min(next.before.start as usize - old_end).min(next.after.start as usize - new_end)
            });
            for offset in 0..context {
                row(
                    DiffLineKind::Context,
                    Some((old_end + offset + 1) as u64),
                    Some((new_end + offset + 1) as u64),
                    after_lines[new_end + offset],
                )
                .map_err(|error| error.to_string())?;
            }
            previous_after = new_end + context;
        }
        output.flush().map_err(|error| error.to_string())?;
        output.get_ref().sync_all().map_err(|error| error.to_string())?;
        Ok(Box::new(WriteFact {
            turn: None, note: None,
            id,
            machine: self.machine.clone(),
            path: path.to_string_lossy().into_owned(),
            kind,
            unavailable: None,
            added: Some(diff.count_additions()),
            removed: Some(diff.count_removals()),
            diff: Some(DiffRef {
                content: Some(crate::files::content_fingerprint(&artifact).map_err(|error| error.to_string())?),
                machine: self.machine.clone(),
                path: artifact.to_string_lossy().into_owned(),
                bytes: Some(written),
                lines: kept,
            }),
        }))
    }
}

fn decode<'a>(bytes: Option<&'a [u8]>, encoding: Option<&str>) -> Result<Cow<'a, str>, String> {
    let Some(bytes) = bytes else { return Ok(Cow::Borrowed("")) };
    let plan = Plan::inspect(&mut io::Cursor::new(bytes), encoding)?;
    if plan.is_utf8() {
        let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
        return Ok(Cow::Borrowed(text.strip_prefix('\u{feff}').unwrap_or(text)));
    }
    let source = Source::decode(bytes.to_vec(), encoding)?;
    if let Some(issue) = source.issue() {
        return Err(issue.to_string());
    }
    Ok(Cow::Owned(source.text))
}
