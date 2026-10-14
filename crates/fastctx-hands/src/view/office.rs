//! The view adapter owns file identity; fastctx-office owns Office content.

use std::fs::{File, Metadata};
use std::io;
use std::path::Path;

use fastctx_types::residents::Seen;
use jiff::Timestamp;

use super::{Options, Section};

pub(super) fn look(
    path: &Path,
    options: &Options,
    budget: usize,
    file: File,
    metadata: &Metadata,
    format: fastctx_office::Format,
) -> io::Result<(Section, Option<Seen>)> {
    if options.image || options.encoding.is_some() || options.chars.is_some() {
        return Ok((
            Section::error(
                path,
                options,
                "Office content is decoded directly; --image, --encoding and --chars are not supported",
            ),
            None,
        ));
    }
    let overhead = Section::note(path, options, "").cost;
    let selection = fastctx_office::Selection {
        lines: options.lines,
        pages: options.pages,
        sheet: options.sheet.clone(),
        sheet_index: options.sheet_index,
        range: options.range.clone(),
        master: options.master,
    };
    if overhead >= budget {
        let mut note = super::not_shown(path, options);
        note.partial = true;
        return Ok((note, None));
    }
    let read = if options.exploring() {
        fastctx_office::explore(file, format, budget - overhead, super::explore::WHOLE.saturating_sub(overhead))
    } else {
        fastctx_office::read(file, format, &selection, budget - overhead)
    };
    let content = match read {
        Ok(content) => content,
        Err(error) => {
            return Ok((
                Section::error(
                    path,
                    options,
                    &format!("Office content could not be read: {}", super::text::visible(&format!("{error:#}"))),
                ),
                None,
            ));
        }
    };
    if content.text.is_empty() && content.stopped {
        let mut note = super::not_shown(path, options);
        note.partial = true;
        return Ok((note, None));
    }
    let mut section = Section::note(path, options, &content.facts);
    section.text.push_str(&content.text);
    section.cost = section.text.len();
    section.shown = true;
    section.partial = content.stopped;
    section.whole = content.whole;
    let seen = Seen {
        path: path.to_string_lossy().into_owned(),
        at: Timestamp::now(),
        sha256: None,
        size: metadata.len(),
        modified: metadata.modified().ok().and_then(|time| Timestamp::try_from(time).ok()),
        encoding: None,
        encoding_selection: None,
    };
    Ok((section, Some(seen)))
}
