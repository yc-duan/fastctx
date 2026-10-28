//! Adapt PDF pages to view's output, budget, image transport and seen contract.

use super::{Options, STANDARD, Section};
use crate::run::window::{IMAGE_COST, bytes};
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
    if options.lines.is_some() || options.chars.is_some() || options.encoding.is_some() {
        return Ok((
            Section::error(path, options, "a PDF; --lines, --chars and --encoding apply to text files; use --pages"),
            None,
        ));
    }
    let section = match Document::open(path) {
        Ok(document) => read(path, options, budget, &document),
        Err(reason) => Section::error(path, options, &reason),
    };
    let seen = section.shown.then(|| Seen {
        path: path.to_string_lossy().into_owned(),
        at: Timestamp::now(),
        sha256: None,
        size: metadata.len(),
        modified: metadata.modified().ok().and_then(|time| Timestamp::try_from(time).ok()),
        encoding: None,
        encoding_selection: None,
    });
    Ok((section, seen))
}

fn read(path: &Path, options: &Options, budget: usize, document: &Document) -> Section {
    if options.exploring() {
        return explore(path, options, budget, document);
    }
    read_selected(path, options, budget, document)
}

fn explore(path: &Path, options: &Options, budget: usize, document: &Document) -> Section {
    let n = u64::MAX;
    let spare = format!(
        "=== page {n} (line {n}: characters 1-{n} of {n} shown; remaining selected content not shown) ===\n"
    )
    .len()
        + format!(
            "=== page {n} (lines 1-{n} of {n}; line {n}: characters 1-{n} of {n} shown; remaining text not shown) ===\n"
        )
        .len();
    let mut whole = read_selected(path, options, budget.min(super::explore::WHOLE + spare), document);
    if !whole.partial && whole.cost <= super::explore::WHOLE {
        whole.whole = true;
        return whole;
    }
    let total = document.page_count() as u64;
    let mut first = options.clone();
    let mut sample = super::explore::Sample::new();
    let outline = document.outline(|title, page, level| {
        let title = super::text::visible(title);
        sample.push(super::explore::Entry {
            number: page.map_or(0, |page| page as u64 + 1),
            from: 1,
            length: title.chars().count() as u64,
            text: if title.len() <= super::explore::WHOLE { title } else { String::new() },
            note: format!(
                "{} (level {})",
                page.map_or("page unknown".into(), |page| format!("page {}", page + 1)),
                level + 1
            ),
        });
    });
    let mut bookmarks = String::new();
    let mut failed = false;
    let mut shown = false;
    let mut marked = false;
    match outline {
        Ok(()) => {
            let longest = format!("=== bookmarks ({n} of {n} entries shown) ===\n", n = u64::MAX);
            let room = budget.min(super::explore::WHOLE - super::explore::BRIEF).saturating_sub(longest.len());
            let (total, entries) = sample.finish(room, super::explore::ENTRIES);
            marked = total > 0;
            let header = if total == 0 {
                "=== bookmarks (none) ===\n".into()
            } else {
                format!("=== bookmarks ({} of {total} entries shown) ===\n", entries.len())
            };
            bookmarks.push_str(&header);
            shown |= !entries.is_empty();
            for entry in entries {
                bookmarks.push_str(&format!("{}\t{}\n", entry.note, entry.text));
            }
        }
        Err(reason) => {
            bookmarks.push_str(&format!("=== bookmarks (not shown: {}) ===\n", super::text::visible(&reason)));
            failed = true;
        }
    }
    // With bookmarks to read by, the start of the first page says what it
    // is; with none, the first two pages are its only map.
    let (pages, room) = if marked { (1, super::explore::BRIEF) } else { (2, super::explore::HEAD) };
    first.pages = Some((1, pages.min(total.max(1))));
    let omitted =
        if total > pages { format!("=== pages {}-{total} (not shown) ===\n", pages + 1) } else { String::new() };
    let head_budget = room.min(budget.saturating_sub(bookmarks.len() + omitted.len()));
    let mut section = read_selected(path, &first, head_budget, document);
    section.append_note(&omitted);
    section.append_note(&bookmarks);
    section.failed |= failed;
    section.shown |= shown;
    section
}

fn read_selected(path: &Path, options: &Options, budget: usize, document: &Document) -> Section {
    let total = document.page_count() as u64;
    let pages = super::plural(total, "page", "pages");
    let (first, asked_last) = options.pages.unwrap_or((1, total));
    if first == 0 || asked_last < first || first > total {
        return Section::error(path, options, &format!("PDF, {pages}; no pages in the range asked"));
    }
    let last = asked_last.min(total);
    let mut section = Section::note(path, options, &format!("PDF, {pages}"));
    // Reserve the longest location note: all counts are at most 20 digits.
    let tail = format!(
        "=== page {n} (line {n}: characters 1-{n} of {n} shown; remaining selected content not shown) ===\n",
        n = u64::MAX
    );
    if section.cost + tail.len() > budget {
        let mut note = super::not_shown(path, options);
        note.partial = true;
        return note;
    }
    let mut left = budget - section.cost - tail.len();
    for number in first..=last {
        if options.image && left < bytes(IMAGE_COST) || left == 0 {
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
        if let Some((width, height)) = part.drawn {
            section.pictures.push(fastctx_types::run_facts::StepPicture {
                path: path.to_string_lossy().into_owned(),
                page: Some(number),
                width,
                height,
                mime: "image/png".into(),
            });
        }
        section.cost += part.cost;
        left -= part.cost;
        section.shown |= part.shown;
        section.failed |= part.failed;
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
        self.cost += note.len();
        self.text.push_str(note);
    }
}

struct PageOutput {
    text: String,
    cost: usize,
    shown: bool,
    partial: bool,
    failed: bool,
    /// The page drawn as a picture, its width and height.
    drawn: Option<(u32, u32)>,
}
impl PageOutput {
    fn note(number: u64, reason: &str) -> Self {
        let text = format!("=== page {number} ({}) ===\n", super::text::visible(reason));
        Self { cost: text.len(), text, shown: false, partial: false, failed: true, drawn: None }
    }
}

fn image(page: ImagePage, number: u64) -> PageOutput {
    let facts = if page.notes.is_empty() {
        String::new()
    } else {
        format!("; {}", super::text::visible(&page.notes.join("; ")))
    };
    let note = format!("=== page {number} (image {}x{}{facts}) ===\n", page.width, page.height);
    // At most 15 white-composited 2000x2000 RGB pages fit the answer's budget;
    // their raw pixels remain below the image transport's 400 MiB allowance.
    let cost = note.len() + bytes(IMAGE_COST) + 1;
    let text = format!("{note}\x1b]1337;File=inline=1;size={}:{}\x07\n", page.png.len(), STANDARD.encode(page.png));
    PageOutput { text, cost, shown: true, partial: false, failed: false, drawn: Some((u32::from(page.width), u32::from(page.height))) }
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
    let mut left = budget.saturating_sub(longest.len());
    let mut body = String::new();
    let mut shown = 0;
    let mut cut = None;
    for (index, line) in lines.iter().enumerate() {
        let prefix = format!("{}\t", index + 1);
        let overhead = prefix.len() + 1;
        if left < overhead {
            break;
        }
        let kept = crate::answer::prefix(line, left - overhead);
        body.push_str(&prefix);
        body.push_str(kept);
        body.push('\n');
        left -= overhead + kept.len();
        shown += 1;
        if kept.len() < line.len() {
            cut = Some((shown, kept.chars().count(), line.chars().count()));
            break;
        }
    }
    let partial = cut.is_some() || shown < lines.len();
    if shown == 0 && !lines.is_empty() {
        // Not even the start of the page fits: it is named among the pages
        // not shown.
        return PageOutput { text: String::new(), cost: 0, shown: false, partial: true, failed: false, drawn: None };
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
    PageOutput { cost: text.len(), text, shown: shown > 0, partial, failed: false, drawn: None }
}
