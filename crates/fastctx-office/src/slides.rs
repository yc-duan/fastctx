use anyhow::{Result, bail};
use office_oxide::Document;

use crate::{Selection, output::Output};

pub(crate) fn read(
    document: &Document,
    source: Option<std::fs::File>,
    selection: &Selection,
    out: &mut Output,
) -> Result<()> {
    if let Some(ppt) = document.as_ppt()
        && !ppt.text_complete()
    {
        bail!("PowerPoint text is incomplete: {}", ppt.warnings().join("; "));
    }
    let mut ir = document.to_ir();
    if ir.metadata.text_truncated {
        bail!("PowerPoint text is incomplete: {}", ir.metadata.warnings.join("; "));
    }
    let mut notes = source.map(crate::notes::Notes::open).transpose()?;
    let total =
        document.as_pptx().map(|d| d.slides.len()).or_else(|| document.as_ppt().map(|d| d.slides.len())).unwrap();
    let mut sections = std::mem::take(&mut ir.sections);
    if sections.len() < total {
        bail!("PowerPoint parser returned fewer slides than the presentation declares");
    }
    ir.sections = sections.split_off(total);
    let master_text = ir.plain_text();
    if !out.line("", &format!("=== PowerPoint ({total} slides) ==="), "use --pages 1") {
        return Ok(());
    }
    let (first, last) = selection.pages.unwrap_or((1, total as u64));
    if first > total as u64 {
        out.line("", "no slides in the range asked", "use --pages 1");
        return Ok(());
    }
    for (i, mut section) in sections.into_iter().enumerate() {
        let number = i as u64 + 1;
        if number < first || number > last {
            continue;
        }
        // Slide titles are already elements; section titles repeat them.
        section.title = None;
        let extra_notes = if let (Some(notes), Some(pptx)) = (&mut notes, document.as_pptx()) {
            section.speaker_notes = None;
            notes.slide(&pptx.presentation.slides[i].rel_id)?
        } else {
            String::new()
        };
        let hidden = section.hidden;
        ir.sections = vec![section];
        let mut text = ir.plain_text();
        if !extra_notes.trim().is_empty() {
            text.push_str("\n\n[Notes]\n");
            text.push_str(extra_notes.trim_end_matches('\n'));
        }
        // File-wide master text belongs to the final slide's line space,
        // so every continuation remains reachable with --pages/--lines.
        if i + 1 == total && !master_text.is_empty() {
            text.push_str("\n\n[Master text]\n");
            text.push_str(&master_text);
        }
        let label = format!("slide {number} of {total}");
        let state = if hidden { "; hidden" } else { "" };
        let resume = format!("use --pages {number}");
        if !out.line("", &format!("=== {label} ({} lines{state}) ===", text.lines().count()), &resume)
            || !out.numbered(&text, selection.lines, Some(number))
        {
            break;
        }
    }
    Ok(())
}
