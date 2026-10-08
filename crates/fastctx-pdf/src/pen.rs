//! Preserve content-stream adjacency before joining positioned fragments.

use super::{bidi, paragraphs};
use hayro::kurbo::{Point, Rect};

pub(super) struct Glyph {
    pub text: Option<String>,
    pub start: Point,
    pub end: Point,
    pub bounds: Rect,
    pub em: f64,
    pub sequence: usize,
}

pub(super) struct Piece {
    pub text: Option<String>,
    pub x: f64,
    pub end: f64,
    pub em: f64,
    pub sequence: usize,
    pub fragment: usize,
    pub synthetic_space: bool,
}

pub(super) struct Line {
    pub text: String,
    pub bounds: Rect,
    pub em: f64,
    pub paragraph: Option<usize>,
}

struct Fragment {
    pieces: Vec<Piece>,
    bounds: Rect,
    baseline: f64,
    base_end: Point,
    em: f64,
    id: usize,
    rtl_step: Option<bool>,
    paragraph: Option<usize>,
    has_text: bool,
}

impl Fragment {
    fn from(glyph: &Glyph) -> Self {
        Self {
            pieces: Vec::new(),
            bounds: glyph.bounds,
            baseline: glyph.start.y,
            base_end: glyph.end,
            em: glyph.em,
            id: glyph.sequence,
            rtl_step: None,
            paragraph: None,
            has_text: false,
        }
    }

    fn add(&mut self, glyph: &Glyph, space: Option<f64>) {
        if let Some(x) = space {
            self.pieces.push(Piece {
                text: Some(" ".into()),
                x,
                end: x,
                em: glyph.em,
                sequence: glyph.sequence,
                fragment: self.id,
                synthetic_space: true,
            });
        }
        self.pieces.push(Piece {
            text: glyph.text.clone(),
            x: glyph.start.x,
            end: glyph.end.x,
            em: glyph.em,
            sequence: glyph.sequence,
            fragment: self.id,
            synthetic_space: false,
        });
        self.bounds = self.bounds.union(glyph.bounds);
        if !blank(glyph.text.as_deref()) {
            if !self.has_text || glyph.em >= self.em {
                self.base_end = glyph.end;
            }
            if !self.has_text || glyph.em > self.em {
                self.em = glyph.em;
                self.baseline = glyph.start.y;
            }
            self.has_text = true;
        }
    }
}

fn blank(text: Option<&str>) -> bool {
    text.is_some_and(|text| text.chars().all(char::is_whitespace))
}

fn space_after(text: Option<&str>) -> bool {
    text.and_then(|text| text.chars().last()).is_some_and(|c| {
        // These scripts use word spaces; CJK ideographs and punctuation do not.
        matches!(u32::from(c), 0x0021..=0x02ff | 0x0370..=0x052f | 0x0590..=0x08ff | 0x1e00..=0x1fff | 0x2000..=0x206f)
            && !c.is_whitespace()
    })
}

fn needs_space(previous: Option<&str>, next: Option<&str>, gap: f64, em: f64) -> bool {
    gap > 0.15 * em && space_after(previous) && !next.is_some_and(|text| text.starts_with(char::is_whitespace))
}

pub(super) fn lines(glyphs: Vec<Glyph>, vertical: bool) -> Vec<Line> {
    let mut fragments = Vec::<Fragment>::new();
    let mut previous: Option<Glyph> = None;
    for glyph in glyphs {
        if !fragments.is_empty()
            && glyph.text.as_ref().is_some_and(|text| {
                !text.is_empty() && text.chars().all(|c| unicode_bidi::bidi_class(c) == unicode_bidi::BidiClass::NSM)
            })
        {
            fragments.last_mut().unwrap().add(&glyph, None);
            continue;
        }
        let mut join = false;
        let mut space = None;
        if let Some(last) = &previous {
            let em = last.em.min(glyph.em);
            let rtl = !vertical && bidi::has_rtl(last.text.as_deref().unwrap_or("")) && glyph.start.x < last.start.x;
            // Use the glyph's own width. dvips combines positive Tc with
            // cancelling TJ adjustments within words; treating Tc as part of
            // the pen endpoint erases its remaining inter-word spaces.
            let gap = if rtl { last.start.x - glyph.end.x } else { glyph.start.x - last.end.x };
            let fragment = fragments.last_mut().unwrap();
            // A superscript followed by a subscript can span more than 0.8 em
            // although both belong to the same base letter. Measure against
            // the fragment's main baseline, not the preceding small script.
            // Wide upper and lower scripts can overlap by more than a small
            // script's advance. Both still attach to the same base endpoint.
            let stacked = last.em < fragment.em
                && glyph.em < fragment.em
                && (last.start.y - fragment.baseline) * (glyph.start.y - fragment.baseline) < 0.0
                && (glyph.start.x - fragment.base_end.x).abs() <= 0.8 * glyph.em;
            join = (glyph.start.y - fragment.baseline).abs() < 0.8 * fragment.em.min(glyph.em)
                && (gap.abs() <= 0.8 * em || stacked);
            let both_rtl = !vertical
                && bidi::has_rtl(last.text.as_deref().unwrap_or(""))
                && bidi::has_rtl(glyph.text.as_deref().unwrap_or(""));
            let forward = glyph.start.x > last.start.x;
            if both_rtl && fragment.rtl_step.is_some_and(|previous| previous != forward) {
                // A producer can emit one shaped cluster visually, then jump
                // back to the next logical cluster. That turn is not a space.
                join = false;
            }
            if join && both_rtl {
                fragment.rtl_step = Some(forward);
            }
            if join && !vertical && needs_space(last.text.as_deref(), glyph.text.as_deref(), gap, em) {
                space = Some(if rtl { (last.start.x + glyph.end.x) / 2.0 } else { (last.end.x + glyph.start.x) / 2.0 });
            }
        }
        if !join {
            fragments.push(Fragment::from(&glyph));
        }
        fragments.last_mut().unwrap().add(&glyph, space);
        previous = Some(glyph);
    }
    // Spaces carry word boundaries, but cannot establish a main baseline or
    // bridge two visible rows. Attach them after the text lines are formed.
    let (spaces, mut fragments): (Vec<_>, Vec<_>) = fragments.into_iter().partition(|part| !part.has_text);
    // Large text anchors the baseline. Small detached scripts can then enter
    // an existing line at their position instead of becoming a preceding row.
    let columns = paragraphs::Columns::find(
        &fragments
            .iter()
            .map(|part| {
                let mut bounds = part.bounds;
                if !vertical {
                    let visible = part
                        .pieces
                        .iter()
                        .filter(|piece| piece.text.as_ref().is_none_or(|text| !text.chars().all(char::is_whitespace)));
                    let (left, right) = visible.fold((f64::INFINITY, f64::NEG_INFINITY), |(left, right), piece| {
                        (left.min(piece.x.min(piece.end)), right.max(piece.x.max(piece.end)))
                    });
                    // Trailing spaces must not conceal a paragraph's actual
                    // right alignment; keep those spaces in the text itself.
                    if left.is_finite() {
                        bounds.x0 = left;
                        bounds.x1 = right;
                    }
                }
                paragraphs::Fragment {
                    bounds,
                    baseline: part.baseline,
                    em: part.em,
                    text: part.pieces.iter().filter_map(|piece| piece.text.as_deref()).collect(),
                }
            })
            .collect::<Vec<_>>(),
    );
    for (part, &paragraph) in fragments.iter_mut().zip(&columns.membership) {
        part.paragraph = paragraph;
    }
    fragments.sort_by(|a, b| {
        b.em.total_cmp(&a.em).then(a.baseline.total_cmp(&b.baseline)).then(a.bounds.x0.total_cmp(&b.bounds.x0))
    });
    let mut joined: Vec<Fragment> = Vec::new();
    for fragment in fragments {
        let nearest = joined
            .iter()
            .enumerate()
            .filter(|(_, line)| !columns.separates(line.paragraph, fragment.paragraph))
            .filter_map(|(index, line)| adjacent(line, &fragment, vertical).map(|delta| (index, delta)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((mut index, _)) = nearest {
            merge(&mut joined[index], fragment, vertical);
            // A later centered glyph can bridge two already-created pieces
            // of one column. Joining must close that connection transitively.
            while let Some(other) = (0..joined.len()).find(|&other| {
                other != index
                    && !columns.separates(joined[index].paragraph, joined[other].paragraph)
                    && adjacent(&joined[index], &joined[other], vertical).is_some()
            }) {
                let fragment = joined.remove(other);
                if other < index {
                    index -= 1;
                }
                merge(&mut joined[index], fragment, vertical);
            }
        } else {
            joined.push(fragment);
        }
    }
    for space in spaces {
        let nearest = joined
            .iter()
            .enumerate()
            .filter_map(|(index, line)| {
                adjacent(line, &space, vertical).map(|delta| {
                    let gap = (line.bounds.x0 - space.bounds.x1).max(space.bounds.x0 - line.bounds.x1).max(0.0);
                    (index, delta, gap)
                })
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(a.2.total_cmp(&b.2)));
        if let Some((index, _, _)) = nearest {
            attach_spaces(&mut joined[index], space.pieces);
        }
    }
    joined
        .into_iter()
        .map(|fragment| Line {
            text: bidi::text(fragment.pieces, vertical),
            bounds: fragment.bounds,
            em: fragment.em,
            paragraph: fragment.paragraph,
        })
        .filter(|line| !line.text.is_empty())
        .collect()
}

fn attach_spaces(line: &mut Fragment, mut spaces: Vec<Piece>) {
    // Some producers paint positioning spaces on top of actual letters.
    // Such a spacer occupies no word gap and must not split the covered word.
    spaces.retain(|space| {
        let middle = (space.x + space.end) / 2.0;
        !line.pieces.iter().any(|piece| {
            let (left, right) = (piece.x.min(piece.end), piece.x.max(piece.end));
            let precision = (left.abs() + right.abs() + middle.abs()) * f64::from(f32::EPSILON);
            !blank(piece.text.as_deref()) && middle > left + precision && middle < right - precision
        })
    });
    if line.pieces.iter().any(|piece| bidi::has_rtl(piece.text.as_deref().unwrap_or(""))) {
        line.pieces.extend(spaces);
        return;
    }
    for space in spaces {
        let middle = (space.x + space.end) / 2.0;
        let mut at = line.pieces.iter().position(|piece| piece.x >= middle).unwrap_or(line.pieces.len());
        // An explicit space replaces the inferred separator in the same gap;
        // other explicit spaces retain their original multiplicity.
        while at > 0 && line.pieces[at - 1].synthetic_space {
            at -= 1;
            line.pieces.remove(at);
        }
        while at < line.pieces.len() && line.pieces[at].synthetic_space {
            line.pieces.remove(at);
        }
        line.pieces.insert(at, space);
    }
}

fn adjacent(line: &Fragment, fragment: &Fragment, vertical: bool) -> Option<f64> {
    let delta = (fragment.baseline - line.baseline).abs();
    let gap = (fragment.bounds.x0 - line.bounds.x1).max(line.bounds.x0 - fragment.bounds.x1).max(0.0);
    let script = fragment.em < line.em - line.em * f64::EPSILON.sqrt()
        && fragment.bounds.y0 < line.bounds.y1
        && fragment.bounds.y1 > line.bounds.y0;
    let tolerance = if script {
        0.8 * fragment.em
    } else if vertical {
        0.5 * line.em
    } else {
        line.em / 3.0
    };
    // The two source coordinates were rounded to f32 before subtraction.
    let precision = (line.bounds.x1.abs() + fragment.bounds.x0.abs()) * f64::from(f32::EPSILON);
    let label = |part: &Fragment| {
        paragraphs::label(&part.pieces.iter().filter_map(|piece| piece.text.as_deref()).collect::<String>())
    };
    // Only an evidenced numbered column can extend the usual gap. A lone
    // parenthesized table value must not connect to text across a page gutter.
    let hanging = !vertical
        && line.paragraph.is_some()
        && line.paragraph == fragment.paragraph
        && (line.bounds.x1 <= fragment.bounds.x0 && label(line)
            || fragment.bounds.x1 <= line.bounds.x0 && label(fragment));
    (delta <= tolerance && (gap <= line.em + precision || hanging)).then_some(delta)
}

fn merge(line: &mut Fragment, mut next: Fragment, vertical: bool) {
    line.paragraph = line.paragraph.or(next.paragraph);
    let before = next.bounds.x1 <= line.bounds.x0;
    let after = next.bounds.x0 >= line.bounds.x1;
    let (left, right) = if before { (&next, &*line) } else { (&*line, &next) };
    let gap = right.bounds.x0 - left.bounds.x1;
    let space = !vertical
        && (before || after)
        && needs_space(
            left.pieces.last().and_then(|piece| piece.text.as_deref()),
            right.pieces.first().and_then(|piece| piece.text.as_deref()),
            gap,
            line.em.min(next.em),
        );
    let separator = space.then(|| Piece {
        text: Some(" ".into()),
        x: (left.bounds.x1 + right.bounds.x0) / 2.0,
        end: (left.bounds.x1 + right.bounds.x0) / 2.0,
        em: line.em,
        sequence: right.pieces.first().unwrap().sequence,
        fragment: right.id,
        synthetic_space: true,
    });
    line.bounds = line.bounds.union(next.bounds);
    if before {
        if let Some(separator) = separator {
            next.pieces.push(separator);
        }
        next.pieces.append(&mut line.pieces);
        line.pieces = next.pieces;
    } else if after {
        if let Some(separator) = separator {
            line.pieces.push(separator);
        }
        line.pieces.append(&mut next.pieces);
    } else {
        // Overlapping fragments include detached super/subscripts. Insert each
        // fragment as a unit; retain the established order inside both pieces.
        let at = line.pieces.iter().position(|piece| piece.x >= next.bounds.x0).unwrap_or(line.pieces.len());
        line.pieces.splice(at..at, next.pieces);
    }
}
