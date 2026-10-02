//! Log snapshots become ordered content blocks; presentation never rewrites stored bytes.

use super::window::Body;
use super::{
    ByteRange, CHARACTER_BUDGET, ContentBlock, JobRecord, JobState, RunResult, images, protocol,
};
use std::fs::File;
use std::io::{self, Read};
use std::path::PathBuf;

/// Reads only `output_bytes` bytes, even when the job continues appending to the same log.
pub fn assemble(
    job: JobRecord,
    record_path: PathBuf,
    output_bytes: u64,
) -> Result<RunResult, String> {
    let result = assemble_inner(job, record_path, output_bytes);
    result.map_err(|error| error.to_string())
}

fn assemble_inner(
    job: JobRecord,
    record_path: PathBuf,
    output_bytes: u64,
) -> io::Result<RunResult> {
    let window = protocol::scan(File::open(&job.output_path)?.take(output_bytes))?;
    assemble_window(job, record_path, output_bytes, window)
}

pub(super) fn assemble_window(
    job: JobRecord,
    record_path: PathBuf,
    output_bytes: u64,
    window: super::window::Window,
) -> io::Result<RunResult> {
    let (header, footer) = notes(&job);
    let budget = CHARACTER_BUDGET
        .checked_sub(header.chars().count() + footer.chars().count())
        .ok_or_else(|| io::Error::other("run head notes exceed the character budget"))?;
    let omission_cost = omission(
        &job,
        &ByteRange {
            start: output_bytes,
            end: output_bytes,
        },
    )
    .chars()
    .count();
    let (pieces, omitted) = window.select(budget, omission_cost);
    enum Prepared {
        Text(String),
        Image(images::Image),
    }
    let mut prepared = Vec::new();
    let mut omission_written = false;
    for piece in pieces {
        if let Some(range) = &omitted
            && !omission_written
            && piece.range.start >= range.end
        {
            prepared.push(Prepared::Text(omission(&job, range)));
            omission_written = true;
        }
        match piece.body {
            Body::Text(text) => prepared.push(Prepared::Text(text)),
            Body::Image {
                protocol,
                complete,
                source,
            } => {
                let decoded = if complete {
                    images::load(&job.output_path, &source, protocol)
                } else {
                    Err(io::Error::other(
                        "sequence ended before the image was complete",
                    ))
                };
                match decoded {
                    Ok(image) => prepared.push(Prepared::Image(image)),
                    Err(error) => prepared.push(Prepared::Text(format!(
                        "\n[Incomplete {} image at bytes {}..{}: {error}]\n",
                        protocol.name(),
                        piece.range.start,
                        piece.range.end
                    ))),
                }
            }
        }
    }
    if let Some(range) = &omitted
        && !omission_written
    {
        prepared.push(Prepared::Text(omission(&job, range)));
    }
    let mut selected: Vec<_> = prepared
        .iter_mut()
        .filter_map(|piece| match piece {
            Prepared::Image(image) => Some(image),
            _ => None,
        })
        .collect();
    images::fit(&mut selected)?;
    let mut content = vec![ContentBlock::Text { text: header }];
    for piece in prepared {
        match piece {
            Prepared::Text(text) => append(&mut content, text),
            Prepared::Image(image) => {
                append(&mut content, image.note());
                content.push(image.block()?);
            }
        }
    }
    append(&mut content, footer);
    Ok(RunResult {
        job,
        record_path,
        output_bytes,
        omitted,
        content,
    })
}

/// Returns the blocks already rendered for the caller; never attaches structuredContent.
pub fn render(result: &RunResult) -> &[ContentBlock] {
    &result.content
}

fn append(content: &mut Vec<ContentBlock>, text: String) {
    if text.is_empty() {
        return;
    }
    if let Some(ContentBlock::Text { text: previous }) = content.last_mut() {
        previous.push_str(&text);
    } else {
        content.push(ContentBlock::Text { text });
    }
}

fn notes(job: &JobRecord) -> (String, String) {
    let status = match job.state {
        JobState::Running | JobState::Starting => format!("still running after 15s; {}", job.id),
        JobState::Exited => format!(
            "exited {}",
            job.exit_code.expect("exited records have a code")
        ),
        JobState::Interrupted => format!("interrupted; {}", job.id),
        JobState::Failed => format!("failed; {}", job.id),
    };
    let mut header = format!("=== run on {} ({status}) ===\n", job.machine);
    if let Some(error) = &job.error {
        header.push_str(&format!("{error}\n"));
    }
    if let Some(error) = &job.hook_error {
        header.push_str(&format!("{error}\n"));
    }
    let footer = if matches!(job.state, JobState::Running | JobState::Starting) {
        format!(
            "\n=== output continues in {} {}; you'll be told when it ends ===\n",
            job.machine,
            crate::paths::display_path(&job.output_path)
        )
    } else {
        String::new()
    };
    (header, footer)
}

fn omission(job: &JobRecord, range: &ByteRange) -> String {
    format!(
        "\n=== omitted log bytes {}..{} (0-based, end exclusive); full output in {} {} ===\n",
        range.start,
        range.end,
        job.machine,
        crate::paths::display_path(&job.output_path)
    )
}
