//! Keep separately painted combining marks attached to their positioned base.

use super::pen::Piece;
use unicode_bidi::{BidiClass, bidi_class};
use unicode_normalization::UnicodeNormalization;

pub(super) fn attach(pieces: &mut Vec<Piece>) {
    let mark = |text: &str| !text.is_empty() && text.chars().all(|c| bidi_class(c) == BidiClass::NSM);
    for index in 0..pieces.len() {
        if !pieces[index].text.as_deref().is_some_and(mark) {
            continue;
        }
        let point = pieces[index].x;
        let distance = |piece: &Piece| (piece.x.min(piece.end) - point).max(point - piece.x.max(piece.end)).max(0.0);
        let parent = pieces
            .iter()
            .enumerate()
            .filter(|(i, piece)| {
                *i != index
                    && piece
                        .text
                        .as_ref()
                        .is_some_and(|text| text.chars().any(|c| !c.is_whitespace() && bidi_class(c) != BidiClass::NSM))
            })
            .min_by(|(_, a), (_, b)| {
                let recent =
                    |piece: &Piece| piece.sequence < pieces[index].sequence && distance(piece) < pieces[index].em * 0.1;
                if recent(a) != recent(b) {
                    return recent(b).cmp(&recent(a));
                }
                if recent(a) {
                    return a
                        .sequence
                        .abs_diff(pieces[index].sequence)
                        .cmp(&b.sequence.abs_diff(pieces[index].sequence));
                }
                distance(a).total_cmp(&distance(b)).then_with(|| {
                    a.sequence.abs_diff(pieces[index].sequence).cmp(&b.sequence.abs_diff(pieces[index].sequence))
                })
            })
            .map(|(index, _)| index);
        let Some(parent) = parent else {
            continue;
        };
        let marks = pieces[index].text.take().unwrap();
        // A ligature's ToUnicode may contain several bases (e.g. lam-alef).
        // Its mark position selects the base within that same glyph cluster.
        let base = &mut pieces[parent];
        let text = base.text.as_ref().unwrap();
        let bases: Vec<_> = text.char_indices().filter(|(_, c)| bidi_class(*c) != BidiClass::NSM).collect();
        let width = base.end - base.x;
        let fraction = if width == 0.0 { 0.0 } else { ((point - base.x) / width).clamp(0.0, 1.0) };
        let fraction = if super::bidi::has_rtl(text) { 1.0 - fraction } else { fraction };
        let at = ((fraction * bases.len() as f64) as usize).min(bases.len() - 1);
        let boundary = bases.get(at + 1).map(|(offset, _)| *offset).unwrap_or(text.len());
        let mut joined = text.clone();
        joined.insert_str(boundary, &marks);
        base.text = Some(joined.nfc().collect());
        pieces[index].text = Some(String::new());
    }
    pieces.retain(|piece| piece.text.as_ref().is_none_or(|text| !text.is_empty()));
}
