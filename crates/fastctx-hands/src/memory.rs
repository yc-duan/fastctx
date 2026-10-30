//! The generated project-memory block shared by session context and project show.

use fastctx_types::memory::{Catalog, Folders, Revision};

use fastctx_types::memory::words;

/// Journal rows, including the omission notice, take at most 2,048 output units.
/// Project attributes and the complete spec index have independent, user-owned sizes.
pub const JOURNAL_INDEX_BYTES: usize = 8_192;

pub fn map(catalog: &Catalog, folders: Result<&Folders, &str>, read_only: bool) -> String {
    let mut text = String::from(words::INTRO);
    match folders {
        Ok(folders) => {
            text.push_str(&format!("Specs ({} on {}):\n", folders.specs, folders.machine));
            specs(&mut text, catalog);
            text.push_str(&format!("Journals ({} on {}), newest first:\n", folders.journals, folders.machine));
        }
        Err(why) => {
            text.push_str(&format!("{} {why}\nSpecs:\n", words::UNAVAILABLE));
            specs(&mut text, catalog);
            text.push_str(fastctx_types::memory::words::JOURNAL_HEADING);
        }
    }
    text.push_str(&journals(catalog));
    text.push_str(words::READ);
    text.push_str(if read_only { words::READ_ONLY } else { words::WRITE });
    text
}

fn specs(text: &mut String, catalog: &Catalog) {
    for spec in &catalog.specs {
        text.push_str(&format!("{}: {}\n", spec.name, spec.description));
    }
}

/// A chronological prefix, never a selection that silently skips a long recent row.
pub fn journals(catalog: &Catalog) -> String {
    let mut text = String::new();
    let count = catalog.journals.len();
    for (index, revision) in catalog.journals.iter().enumerate() {
        let row = format!(
            "{}: {} ({})\n",
            revision.name,
            revision.description,
            fastctx_types::memory::date(revision.author.stamp.at)
        );
        let remaining = count - index - 1;
        let tail = if remaining == 0 { String::new() } else { words::older(remaining) };
        if text.len() + row.len() + tail.len() > JOURNAL_INDEX_BYTES {
            text.push_str(&words::older(count - index));
            return text;
        }
        text.push_str(&row);
    }
    text
}

pub fn changed(revision: &Revision) -> String {
    let by = match &revision.author.stamp.by {
        fastctx_types::project::Writer::Resident { address } => address.resident.to_string(),
        by => crate::town::writer(by, revision.author.stamp.on.as_ref()),
    };
    words::changed(&by, revision)
}
