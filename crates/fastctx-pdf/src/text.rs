//! Feed original PDF glyph events into content-stream and page-layout ordering.

use super::TextPage;
use super::layout::{Letter, reading_order};
use hayro::hayro_interpret::TransformExt;
use hayro::hayro_syntax::page::Page;
use hayro::kurbo::{Affine, Point, Rect, Vec2};
use std::collections::HashMap;

pub(super) fn read(document: &pdf_oxide::PdfDocument, index: usize, page: &Page<'_>) -> Result<TextPage, String> {
    let events = document.extract_raw_glyphs(index).map_err(|error| error.to_string())?;
    let frame = page.initial_transform(true).to_kurbo();
    let (width, height) = page.render_dimensions();
    let viewport = Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
    let mut letters: Vec<Letter> = Vec::new();
    let mut seen: HashMap<(usize, u32), Vec<usize>> = HashMap::new();
    for event in events {
        let transform = frame * Affine::new(event.matrix.map(f64::from));
        let origin = transform * Point::ZERO;
        let em = ((transform * Point::new(0.0, 1000.0)) - origin).hypot();
        if !origin.x.is_finite()
            || !origin.y.is_finite()
            || !em.is_finite()
            || event.advance.iter().chain(event.rendered_advance.iter()).any(|number| !number.is_finite())
        {
            return Err("text has non-finite coordinates".into());
        }
        if em == 0.0 {
            continue;
        }
        // Respect the declared page viewport, while retaining invisible OCR
        // inside it. Printers' slugs outside a cropped page are not page text.
        let corners = event.corners.map(|[x, y]| frame * Point::new(f64::from(x), f64::from(y)));
        let bounds = corners[1..]
            .iter()
            .fold(Rect::from_points(corners[0], corners[0]), |bounds, point| bounds.union_pt(*point));
        if !viewport.contains(origin) && bounds.intersect(viewport).area() == 0.0 {
            continue;
        }
        let previous = seen.entry((event.font_id, event.char_code)).or_default();
        // Small same-glyph overprints synthesize bold, not additional text.
        if previous.iter().any(|&index| {
            let old = &letters[index];
            old.text == event.text
                && (old.transform * Point::ZERO - origin).hypot() < 0.1 * em
                && old.transform.as_coeffs()[..4] == transform.as_coeffs()[..4]
        }) {
            continue;
        }
        previous.push(letters.len());
        letters.push(Letter {
            text: event.text.map(unligated),
            transform,
            advance: Vec2::new(f64::from(event.advance[0]), f64::from(event.advance[1])),
            cursor: frame * Point::new(f64::from(event.rendered_advance[0]), f64::from(event.rendered_advance[1]))
                - frame * Point::ZERO,
            vertical: event.vertical,
            direction: None,
            sequence: letters.len(),
        });
    }
    let unmapped = letters.iter().filter(|glyph| glyph.text.is_none()).count();
    let lines = if unmapped > 0 && unmapped == letters.len() {
        // The page note carries the exact count. Repeating an unknown marker
        // at every geometric position conveys no additional readable content.
        vec!["[unmapped]".into()]
    } else {
        reading_order(letters)
    };
    let mut notes = Vec::new();
    if unmapped > 0 {
        notes.push(format!("{unmapped} glyphs have no Unicode mapping, shown as [unmapped]"));
    }
    if lines.is_empty() {
        notes.push("no extractable text".into());
    }
    Ok(TextPage { lines, notes })
}

/// The letters of a typographic ligature (U+FB00–FB06), which exist only for
/// layout: text searched for "find" must match a page that drew "ﬁnd".
fn unligated(text: String) -> String {
    if !text.chars().any(|c| ('\u{FB00}'..='\u{FB06}').contains(&c)) {
        return text;
    }
    text.chars()
        .map(|c| match c {
            '\u{FB00}' => "ff".to_string(),
            '\u{FB01}' => "fi".to_string(),
            '\u{FB02}' => "fl".to_string(),
            '\u{FB03}' => "ffi".to_string(),
            '\u{FB04}' => "ffl".to_string(),
            '\u{FB05}' => "\u{17F}t".to_string(),
            '\u{FB06}' => "st".to_string(),
            other => other.to_string(),
        })
        .collect()
}
