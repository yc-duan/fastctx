//! Adapt PDF pages to view's output, budget, image transport and seen contract.

use super::{Options, STANDARD, Section};
use crate::run::window::IMAGE_COST;
use base64::Engine as _;
use fastctx_pdf::{Document, ImagePage, TextPage};
use fastctx_types::residents::Seen;
use jiff::Timestamp;
use std::fs::{File, Metadata};
use std::io;
use std::path::Path;

pub(super) fn look(
    path: &Path,
    options: &Options,
    budget: usize,
    _file: File,
    metadata: &Metadata,
) -> io::Result<(Section, Option<Seen>)> {
    if options.lines.is_some() || options.encoding.is_some() {
        return Ok((
            Section::note(path, options, "a PDF; --lines and --encoding apply to text files; use --pages"),
            None,
        ));
    }
    let section = match Document::open(path) {
        Ok(document) => read(path, options, budget, &document),
        Err(reason) => Section::note(path, options, &reason),
    };
    let seen = section.shown.then(|| Seen {
        path: path.to_string_lossy().into_owned(),
        at: Timestamp::now(),
        sha256: None,
        size: metadata.len(),
        modified: metadata.modified().ok().and_then(|time| Timestamp::try_from(time).ok()),
        encoding: None,
    });
    Ok((section, seen))
}

fn read(path: &Path, options: &Options, budget: usize, document: &Document) -> Section {
    let total = document.page_count() as u64;
    let pages = super::plural(total, "page", "pages");
    let (first, asked_last) = options.pages.unwrap_or((1, total));
    if first == 0 || asked_last < first || first > total {
        return Section::note(path, options, &format!("PDF, {pages}; no pages in the range asked"));
    }
    let last = asked_last.min(total);
    let mut section = Section::note(path, options, &format!("PDF, {pages}"));
    // Reserve the longest location note: all counts are at most 20 digits.
    let tail = format!(
        "=== page {n} (line {n}: characters 1-{n} of {n} shown; remaining selected content not shown) ===\n",
        n = u64::MAX
    );
    if section.cost + tail.chars().count() > budget {
        let mut note = super::not_shown(path, options);
        note.partial = true;
        return note;
    }
    let mut left = budget - section.cost - tail.chars().count();
    for number in first..=last {
        if options.image && left < IMAGE_COST || left == 0 {
            section.append_note(&format!("=== pages {number}-{last} (not shown) ===\n"));
            section.partial = true;
            break;
        }
        let part = if options.image {
            document.render((number - 1) as usize, 2000).map(|page| image(page, number))
        } else {
            document.text((number - 1) as usize).map(|page| text(page, number, left))
        }
        .unwrap_or_else(|reason| PageOutput::note(number, &reason));
        if part.cost > left || part.text.is_empty() {
            section.append_note(&format!("=== pages {number}-{last} (not shown) ===\n"));
            section.partial = true;
            break;
        }
        section.text.push_str(&part.text);
        section.cost += part.cost;
        left -= part.cost;
        section.shown |= part.shown;
        if part.partial {
            if number < last {
                section.append_note(&format!("=== pages {}-{last} (not shown) ===\n", number + 1));
            }
            section.partial = true;
            break;
        }
    }
    section
}

impl Section {
    fn append_note(&mut self, note: &str) {
        self.cost += note.chars().count();
        self.text.push_str(note);
    }
}

struct PageOutput {
    text: String,
    cost: usize,
    shown: bool,
    partial: bool,
}
impl PageOutput {
    fn note(number: u64, reason: &str) -> Self {
        let text = format!("=== page {number} ({}) ===\n", super::text::visible(reason));
        Self { cost: text.chars().count(), text, shown: false, partial: false }
    }
}

fn image(page: ImagePage, number: u64) -> PageOutput {
    let facts = if page.notes.is_empty() {
        String::new()
    } else {
        format!("; {}", super::text::visible(&page.notes.join("; ")))
    };
    let note = format!("=== page {number} (image {}x{}{facts}) ===\n", page.width, page.height);
    // At most 28 white-composited 2000x2000 RGB pages fit the character budget;
    // their raw pixels remain below the image transport's 400 MiB allowance.
    let cost = note.chars().count() + IMAGE_COST + 1;
    let text = format!("{note}\x1b]1337;File=inline=1;size={}:{}\x07\n", page.png.len(), STANDARD.encode(page.png));
    PageOutput { text, cost, shown: true, partial: false }
}

fn text(page: TextPage, number: u64, budget: usize) -> PageOutput {
    let warning = if page.notes.is_empty() {
        String::new()
    } else {
        format!("; {}", super::text::visible(&page.notes.join("; ")))
    };
    let lines: Vec<_> = page.lines.iter().map(|line| super::text::visible(line)).collect();
    if lines.is_empty() {
        return PageOutput::note(number, &page.notes.join("; "));
    }
    let longest = format!(
        "=== page {number} (lines 1-{n} of {n}; line {n}: characters 1-{n} of {n} shown; remaining text not shown{warning}) ===\n",
        n = usize::MAX
    );
    let mut left = budget.saturating_sub(longest.chars().count());
    let mut body = String::new();
    let mut shown = 0;
    let mut cut = None;
    for (index, line) in lines.iter().enumerate() {
        let prefix = format!("{}\t", index + 1);
        let length = line.chars().count();
        let overhead = prefix.len() + 1;
        if left < overhead {
            break;
        }
        let take = length.min(left - overhead);
        body.push_str(&prefix);
        body.extend(line.chars().take(take));
        body.push('\n');
        left -= overhead + take;
        shown += 1;
        if take < length {
            cut = Some((shown, take, length));
            break;
        }
    }
    let partial = cut.is_some() || shown < lines.len();
    if shown == 0 && !lines.is_empty() {
        let mut result = PageOutput::note(number, "text not shown");
        result.partial = true;
        return result;
    }
    let mut facts = format!("lines 1-{shown} of {}", lines.len());
    if let Some((line, take, length)) = cut {
        facts.push_str(&format!("; line {line}: characters 1-{take} of {length} shown"));
    }
    if partial {
        facts.push_str("; remaining text not shown");
    }
    facts.push_str(&warning);
    let text = format!("=== page {number} ({facts}) ===\n{body}");
    PageOutput { cost: text.chars().count(), text, shown: shown > 0, partial }
}
