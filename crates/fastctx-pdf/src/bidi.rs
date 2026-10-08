//! Recover common PDF visual runs while retaining evidenced logical paint order.

use super::pen::Piece;
use unicode_bidi::{BidiClass, BidiInfo, Level, bidi_class};

pub(super) fn has_rtl(text: &str) -> bool {
    text.chars().any(|c| matches!(bidi_class(c), BidiClass::R | BidiClass::AL))
}

pub(super) fn text(mut pieces: Vec<Piece>, vertical: bool) -> String {
    super::marks::attach(&mut pieces);
    if !vertical && pieces.iter().any(|piece| has_rtl(piece.text.as_deref().unwrap_or(""))) {
        let spaces: Vec<_> = pieces
            .iter()
            .filter(|piece| piece.text.as_ref().is_some_and(|text| text.chars().all(char::is_whitespace)))
            .map(|piece| (piece.x.min(piece.end), piece.x.max(piece.end)))
            .collect();
        // A PDF may paint logical text or already reordered visual text. Test
        // the former against the actual positions using the forward UBA.
        let mut emitted: Vec<usize> = (0..pieces.len()).collect();
        emitted.sort_by_key(|&index| pieces[index].sequence);
        let mut visual = emitted.clone();
        visual.sort_by(|&a, &b| pieces[a].x.total_cmp(&pieces[b].x));
        let rtl =
            pieces.iter().flat_map(|piece| piece.text.as_deref().unwrap_or("").chars()).fold(0_i64, |count, c| {
                count
                    + match bidi_class(c) {
                        BidiClass::R | BidiClass::AL => 1,
                        BidiClass::L => -1,
                        _ => 0,
                    }
            }) > 0;
        let base = if rtl { Level::rtl() } else { Level::ltr() };
        let visible = |order: &[usize]| {
            order
                .iter()
                .copied()
                .filter(|&index| pieces[index].text.as_ref().is_none_or(|text| !text.chars().all(char::is_whitespace)))
                .collect::<Vec<_>>()
        };
        // Producers often paint words in logical order but glyphs within each
        // word visually. The pen fragments retain that distinction, including
        // numbers embedded in an RTL phrase inside an LTR paragraph.
        let mut fragments = Vec::new();
        let mut start = 0;
        while start < emitted.len() {
            let id = pieces[emitted[start]].fragment;
            let end = (start + 1..emitted.len())
                .find(|&index| pieces[emitted[index]].fragment != id)
                .unwrap_or(emitted.len());
            let run = &emitted[start..end];
            if run.iter().any(|&index| has_rtl(pieces[index].text.as_deref().unwrap_or(""))) {
                let first = pieces[run[0]].x;
                let last = pieces[*run.last().unwrap()].x;
                if last > first {
                    fragments.extend(reorder(&pieces, run, Level::rtl()));
                } else {
                    fragments.extend_from_slice(run);
                }
            } else {
                fragments.extend_from_slice(run);
            }
            start = end;
        }
        let mut chosen = None;
        for candidate in [&emitted, &fragments] {
            for level in [base, if base.is_rtl() { Level::ltr() } else { Level::rtl() }] {
                if visible(&reorder(&pieces, candidate, level)) == visible(&visual) {
                    chosen = Some((candidate.clone(), level));
                    break;
                }
            }
            if chosen.is_some() {
                break;
            }
        }
        let (order, level) = chosen.unwrap_or_else(|| (reorder(&pieces, &visual, base), base));
        // PDF glyph mappings describe the painted bracket. Undo UBA L4 in
        // RTL runs so the returned logical text has logical opening/closing.
        for (&index, level) in order.iter().zip(levels(&pieces, &order, level)) {
            if level.is_rtl()
                && let Some(text) = &mut pieces[index].text
            {
                *text = text.chars().map(|c| unicode_bidi_mirroring::get_mirrored(c).unwrap_or(c)).collect();
            }
        }
        let mut slots: Vec<_> = pieces.into_iter().map(Some).collect();
        pieces = order.into_iter().map(|index| slots[index].take().unwrap()).collect();
        restore_spaces(&mut pieces, &spaces, level);
    }
    let mut output = String::new();
    let mut missing = 0;
    let mut spaces = String::new();
    for piece in pieces {
        if let Some(text) = piece.text {
            if missing > 0 && text.chars().all(char::is_whitespace) {
                spaces.push_str(&text);
                continue;
            }
            flush_missing(&mut output, &mut missing);
            output.push_str(&spaces);
            spaces.clear();
            output.push_str(&text);
        } else {
            missing += 1;
            spaces.clear();
        }
    }
    flush_missing(&mut output, &mut missing);
    let mut output = output.trim().to_owned();
    if output.ends_with('\u{ad}') {
        output.pop();
        output.push('-');
    }
    output
}

fn restore_spaces(pieces: &mut Vec<Piece>, spaces: &[(f64, f64)], base: Level) {
    pieces.retain(|piece| piece.text.as_ref().is_none_or(|text| !text.chars().all(char::is_whitespace)));
    let order: Vec<_> = (0..pieces.len()).collect();
    let levels = levels(pieces, &order, base);
    let mut groups = Vec::<(f64, f64)>::new();
    let mut group_of = Vec::new();
    let mut previous = None;
    for (piece, level) in pieces.iter().zip(levels) {
        let (lo, hi) = (piece.x.min(piece.end), piece.x.max(piece.end));
        if previous == Some(level) {
            let group = groups.last_mut().unwrap();
            group.0 = group.0.min(lo);
            group.1 = group.1.max(hi);
        } else {
            groups.push((lo, hi));
        }
        group_of.push(groups.len() - 1);
        previous = Some(level);
    }
    let mut insert = Vec::new();
    for index in 1..pieces.len() {
        let a = &pieces[index - 1];
        let b = &pieces[index];
        let (a_bounds, b_bounds) = if group_of[index - 1] == group_of[index] {
            ((a.x.min(a.end), a.x.max(a.end)), (b.x.min(b.end), b.x.max(b.end)))
        } else {
            (groups[group_of[index - 1]], groups[group_of[index]])
        };
        let (lo, hi) = if a_bounds.1 <= b_bounds.0 {
            (a_bounds.1, b_bounds.0)
        } else if b_bounds.1 <= a_bounds.0 {
            (b_bounds.1, a_bounds.0)
        } else {
            continue;
        };
        // Adjacent glyph coordinates were rounded to f32. A tiny numerical
        // seam inside a shaped word is not a location for a space barrier.
        if hi - lo <= (hi.abs() + lo.abs()) * f64::from(f32::EPSILON) {
            continue;
        }
        let closes =
            b.text.as_ref().and_then(|text| text.chars().next()).is_some_and(|c| ",.;:!?%)]}»”،؛؟".contains(c));
        let opens = a.text.as_ref().and_then(|text| text.chars().last()).is_some_and(|c| "([{«“".contains(c));
        if !closes && !opens && spaces.iter().any(|&(start, end)| start <= hi && end >= lo) {
            insert.push(index);
        }
    }
    for index in insert.into_iter().rev() {
        let right = &pieces[index];
        let space = Piece {
            text: Some(" ".into()),
            x: right.x,
            end: right.x,
            em: right.em,
            sequence: right.sequence,
            fragment: right.fragment,
            synthetic_space: true,
        };
        pieces.insert(index, space);
    }
}

fn reorder(pieces: &[Piece], order: &[usize], base: Level) -> Vec<usize> {
    BidiInfo::reorder_visual(&levels(pieces, order, base)).into_iter().map(|index| order[index]).collect()
}

fn levels(pieces: &[Piece], order: &[usize], base: Level) -> Vec<Level> {
    let mut text = String::new();
    let mut offsets = Vec::new();
    for &index in order {
        offsets.push(text.len());
        text.push_str(pieces[index].text.as_deref().filter(|text| !text.is_empty()).unwrap_or("\u{fffc}"));
    }
    let info = BidiInfo::new(&text, Some(base));
    // Glyph mappings can contain several characters (ligatures). Their units
    // stay intact; reversing their scalar values corrupts the mapping itself.
    offsets.iter().map(|&offset| info.levels[offset]).collect()
}

fn flush_missing(output: &mut String, count: &mut usize) {
    match *count {
        0 => {}
        1 => output.push_str("[unmapped]"),
        n => output.push_str(&format!("[unmapped: {n} glyphs]")),
    }
    *count = 0;
}
