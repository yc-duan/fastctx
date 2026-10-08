//! Reconstruct reading order from positioned Unicode glyphs, not paint order.

use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps, InterpreterCache,
    InterpreterSettings, SoftMask, TransformExt, interpret_page,
};
use hayro::hayro_syntax::page::Page;
use hayro::kurbo::{Affine, BezPath, Point, Rect};

use super::PageOutput;

pub(super) fn read(
    page: &Page<'_>,
    settings: &InterpreterSettings,
    number: u64,
    budget: usize,
) -> Result<PageOutput, String> {
    let (width, height) = page.render_dimensions();
    let cache = InterpreterCache::new();
    let mut context = Context::new(
        page.initial_transform(true).to_kurbo(),
        Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
        &cache,
        page.xref(),
        settings.clone(),
    );
    let mut device = Text::default();
    interpret_page(page, &mut context, &mut device);
    if device.glyphs.is_empty() && device.unmapped == 0 {
        return Ok(PageOutput::note(number, "no extractable text"));
    }
    let unmapped = device.unmapped;
    let lines = reading_order(device.glyphs);
    let warning = if unmapped == 0 {
        String::new()
    } else {
        format!("; {unmapped} glyphs have no Unicode mapping, shown as [unmapped]")
    };
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
        return Ok(result);
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
    Ok(PageOutput { cost: text.chars().count(), text, shown: shown > 0, partial })
}

#[derive(Default)]
struct Text {
    glyphs: Vec<Letter>,
    unmapped: usize,
}

struct Letter {
    text: String,
    bounds: Rect,
    baseline: f64,
    em: f64,
    origin: Point,
    angle: f64,
}

impl<'a> Device<'a> for Text {
    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        for glyph in run.glyphs() {
            let transform = props.transform * glyph.transform();
            let origin = transform * Point::ZERO;
            // Hayro normalizes glyph coordinates to 1000 units per em.
            let em = ((transform * Point::new(0.0, 1000.0)) - origin).hypot();
            let direction = (transform * Point::new(1000.0, 0.0)) - origin;
            let advance = match &**glyph {
                Glyph::Outline(glyph) => glyph.advance_width().map(f64::from),
                Glyph::Type3(_) => None,
            }
            .unwrap_or(1000.0);
            let bounds = transform.transform_rect_bbox(Rect::new(0.0, -200.0, advance, 800.0));
            let text = match glyph.as_unicode() {
                Some(BfString::Char(character)) => character.to_string(),
                Some(BfString::String(text)) => text,
                None => {
                    self.unmapped += 1;
                    "[unmapped]".into()
                }
            };
            if bounds.is_finite() && em.is_finite() && !text.is_empty() {
                self.glyphs.push(Letter {
                    text,
                    bounds,
                    baseline: origin.y,
                    em,
                    origin,
                    angle: direction.y.atan2(direction.x),
                });
            }
        }
    }
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

struct Word {
    text: String,
    bounds: Rect,
    em: f64,
    row: usize,
}

fn reading_order(mut letters: Vec<Letter>) -> Vec<String> {
    // Normalize the dominant writing direction before arranging lines. Page
    // rotation and a rotated content stream can both turn normal text sideways.
    let mut orientations = [0usize; 4];
    for letter in &letters {
        let quadrant = (letter.angle / std::f64::consts::FRAC_PI_2).round() as i32;
        orientations[quadrant.rem_euclid(4) as usize] += 1;
    }
    let quadrant = orientations.iter().enumerate().max_by_key(|(_, count)| *count).map_or(0, |(i, _)| i);
    let rotation = Affine::rotate(-(quadrant as f64) * std::f64::consts::FRAC_PI_2);
    for letter in &mut letters {
        letter.bounds = rotation.transform_rect_bbox(letter.bounds);
        letter.baseline = (rotation * letter.origin).y;
    }
    letters.sort_by(|a, b| a.baseline.total_cmp(&b.baseline).then(a.bounds.x0.total_cmp(&b.bounds.x0)));
    // Fill-and-stroke emits the same glyph twice. Exact overprints represent
    // one text character, not two adjacent characters in the text layer.
    letters.dedup_by(|a, b| a.text == b.text && a.bounds == b.bounds);
    let mut words = Vec::new();
    let mut row = Vec::new();
    let mut row_number = 0;
    let mut baseline = 0.0;
    for letter in letters {
        // A baseline displacement below one third em accommodates superscripts
        // and mixed fonts without merging normal adjacent lines.
        if !row.is_empty() && (letter.baseline - baseline).abs() > letter.em / 3.0 {
            make_words(std::mem::take(&mut row), &mut words, row_number);
            row_number += 1;
        }
        if row.is_empty() {
            baseline = letter.baseline;
        }
        row.push(letter);
    }
    make_words(row, &mut words, row_number);

    // Recursive XY cuts expressed as a work stack: full-height whitespace
    // separates columns; horizontal whitespace separates headings and rows.
    // The threshold is measured in the text's own em, not page pixels.
    let mut pending = vec![words];
    let mut lines = Vec::new();
    while let Some(mut region) = pending.pop() {
        if region.is_empty() {
            continue;
        }
        let em = region.iter().map(|w| w.em).sum::<f64>() / region.len() as f64;
        let bounds = region.iter().skip(1).fold(region[0].bounds, |bounds, word| bounds.union(word.bounds));
        // A gap on a single text row separates words, not page columns.
        let vertical = gap(&region, true).filter(|(_, width)| *width > em && bounds.height() > 1.5 * em);
        let horizontal = gap(&region, false);
        let cut = match (vertical, horizontal) {
            (Some((x, width)), Some((y, height))) => Some(if width > height { (true, x) } else { (false, y) }),
            (Some((x, _)), None) => Some((true, x)),
            (None, Some((y, _))) => Some((false, y)),
            (None, None) => None,
        };
        if let Some((vertical, at)) = cut {
            let (first, second): (Vec<_>, Vec<_>) =
                region.into_iter().partition(|word| if vertical { word.bounds.x0 < at } else { word.bounds.y0 < at });
            pending.push(second);
            pending.push(first);
        } else {
            region.sort_by(|a, b| a.row.cmp(&b.row).then(a.bounds.x0.total_cmp(&b.bounds.x0)));
            let mut row = None;
            // Tight leading can make adjacent em boxes overlap. The baselines
            // still identify separate lines even when there is no empty strip.
            for word in region {
                if row == Some(word.row) {
                    let line: &mut String = lines.last_mut().unwrap();
                    line.push(' ');
                    line.push_str(&word.text);
                } else {
                    row = Some(word.row);
                    lines.push(word.text);
                }
            }
        }
    }
    lines
}

fn make_words(mut row: Vec<Letter>, words: &mut Vec<Word>, row_number: usize) {
    row.sort_by(|a, b| a.bounds.x0.total_cmp(&b.bounds.x0));
    let mut current: Option<Word> = None;
    for letter in row {
        let space = letter.text.chars().all(char::is_whitespace);
        // PDFs often encode spaces as TJ offsets instead of Unicode. Half of
        // a typical quarter-em space separates those from ordinary kerning,
        // including the compressed spaces in justified typeset paragraphs.
        let separated = current.as_ref().is_some_and(|word| letter.bounds.x0 - word.bounds.x1 > letter.em / 8.0);
        if (space || separated)
            && let Some(word) = current.take()
        {
            words.push(word);
        }
        if space {
            continue;
        }
        if let Some(word) = &mut current {
            word.text.push_str(&letter.text);
            word.bounds = word.bounds.union(letter.bounds);
        } else {
            current = Some(Word { text: letter.text, bounds: letter.bounds, em: letter.em, row: row_number });
        }
    }
    if let Some(word) = current {
        words.push(word);
    }
}

fn gap(words: &[Word], vertical: bool) -> Option<(f64, f64)> {
    let mut intervals: Vec<_> =
        words.iter().map(|w| if vertical { (w.bounds.x0, w.bounds.x1) } else { (w.bounds.y0, w.bounds.y1) }).collect();
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut end = intervals.first()?.1;
    let mut widest = None;
    for (start, next_end) in intervals.into_iter().skip(1) {
        let width = start - end;
        if width > 0.0 && widest.is_none_or(|(_, previous)| width > previous) {
            widest = Some(((start + end) / 2.0, width));
        }
        end = end.max(next_end);
    }
    widest
}
