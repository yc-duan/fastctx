//! Arrange each writing direction independently before placing page annotations.

use super::pen::{self, Line};
use hayro::kurbo::{Affine, Point, Rect, Vec2};

pub(super) struct Letter {
    pub text: Option<String>,
    pub transform: Affine,
    pub advance: Vec2,
    pub cursor: Vec2,
    pub vertical: bool,
    pub direction: Option<f64>,
    pub sequence: usize,
}

impl Letter {
    fn angle(&self) -> f64 {
        if let Some(angle) = self.direction {
            return angle;
        }
        let [a, b, c, d, _, _] = self.transform.as_coeffs();
        if self.vertical { (-d).atan2(-c) } else { b.atan2(a) }
    }

    fn glyph_angle(&self) -> f64 {
        let [a, b, _, _, _, _] = self.transform.as_coeffs();
        b.atan2(a)
    }
}

struct Flow {
    angle: f64,
    vertical: bool,
    letters: Vec<Letter>,
}

impl Flow {
    fn weight(&self) -> usize {
        self.letters
            .iter()
            .filter(|letter| letter.text.as_ref().is_none_or(|text| !text.chars().all(char::is_whitespace)))
            .count()
    }
}

fn parallel(first: f64, second: f64) -> bool {
    let (sin, cos) = (first - second).sin_cos();
    // Permit rounding in equivalent affine transforms, without merging visibly
    // different text directions or treating opposing baselines as one direction.
    // Source matrices use f32; their two-component affine products introduce
    // four rounded arithmetic operations before the angle is measured.
    sin.abs() <= 4.0 * f64::from(f32::EPSILON) && cos > 0.0
}

pub(super) fn reading_order(mut letters: Vec<Letter>) -> Vec<String> {
    if letters.is_empty() {
        return Vec::new();
    }
    infer_positioned_columns(&mut letters);
    letters.sort_by(|a, b| a.angle().total_cmp(&b.angle()));
    let mut flows: Vec<Flow> = Vec::new();
    for letter in letters {
        let angle = letter.angle();
        if let Some(flow) =
            flows.iter_mut().find(|flow| flow.vertical == letter.vertical && parallel(angle, flow.angle))
        {
            flow.letters.push(letter);
        } else {
            flows.push(Flow { angle, vertical: letter.vertical, letters: vec![letter] });
        }
    }
    let primary = flows
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.weight().cmp(&b.weight()).then_with(|| b.angle.abs().total_cmp(&a.angle.abs())))
        .unwrap()
        .0;
    let primary = flows.remove(primary);
    // Upright vertical CJK advances down the page, but its page still has a
    // top and a left margin. Use glyph orientation for placement, and writing
    // direction for joining letters and ordering the right-to-left columns.
    let vertical = primary.letters.iter().filter(|letter| letter.vertical).count() > primary.letters.len() / 2;
    let mut orientations: Vec<(f64, usize)> = Vec::new();
    for letter in primary.letters.iter().filter(|letter| letter.vertical == vertical) {
        let angle = letter.glyph_angle();
        if let Some((_, count)) = orientations.iter_mut().find(|(other, _)| parallel(*other, angle)) {
            *count += 1;
        } else {
            orientations.push((angle, 1));
        }
    }
    let page_angle = orientations.into_iter().max_by_key(|(_, count)| *count).unwrap().0;
    let page_frame = Affine::rotate(-page_angle);
    let main = primary.lines(page_frame);
    let Some(first) = main.first() else { return Vec::new() };
    let body = main.iter().skip(1).fold(first.bounds, |bounds, line| bounds.union(line.bounds));
    let precision = body.width().max(body.height()) * f64::EPSILON.sqrt();
    let mut insertions: Vec<Vec<String>> = (0..=main.len()).map(|_| Vec::new()).collect();
    let mut margins = Vec::new();
    for flow in flows {
        let along_page = parallel(flow.angle, page_angle);
        let mut previous = 0;
        for line in flow.lines(page_frame) {
            // A side inscription outside the body is ancillary to it. It must
            // neither become the page's first column nor split body paragraphs.
            if !along_page && (line.bounds.x1 <= body.x0 || line.bounds.x0 >= body.x1) {
                margins.push(line.text);
                continue;
            }
            if line.bounds.y1 <= body.y0 {
                insertions[0].push(line.text);
                continue;
            }
            if line.bounds.y0 >= body.y1 {
                insertions[main.len()].push(line.text);
                continue;
            }
            let nearest = main
                .iter()
                .enumerate()
                .min_by(|(i, a), (j, b)| {
                    let da = distance(line.bounds, a.bounds);
                    let db = distance(line.bounds, b.bounds);
                    if (da - db).abs() > precision {
                        da.total_cmp(&db)
                    } else if line.bounds.y0 >= a.bounds.y1 && line.bounds.y0 >= b.bounds.y1 {
                        j.cmp(i)
                    } else {
                        i.cmp(j)
                    }
                })
                .unwrap()
                .0;
            let after = usize::from(line.bounds.y0 > main[nearest].bounds.y0 + precision);
            let at = (nearest + after).max(previous);
            insertions[at].push(line.text);
            previous = at;
        }
    }
    let mut result = Vec::new();
    for (index, line) in main.into_iter().enumerate() {
        result.append(&mut insertions[index]);
        result.push(line.text);
    }
    result.append(insertions.last_mut().unwrap());
    result.extend(margins);
    result
}

fn infer_positioned_columns(letters: &mut [Letter]) {
    // CSS exporters can lay out upright vertical CJK with horizontal simple
    // fonts and a separate position per glyph. WMode then describes the font,
    // not the column. Consecutive ideographs advancing down one em identify
    // that column; keep their original glyph advances for the pen stage.
    let cjk = |letter: &Letter| {
        letter.text.as_ref().is_some_and(|text| !text.is_empty()
        && text.chars().all(|c| matches!(u32::from(c),0x2e80..=0x9fff | 0xac00..=0xd7ff | 0xf900..=0xfaff | 0xfe30..=0xfe4f | 0xff01..=0xff60 | 0x20000..=0x323af)))
    };
    let punctuation =
        |letter: &Letter| letter.text.as_ref().is_some_and(|text| text.chars().all(|c| !c.is_alphanumeric()));
    let mut columns = vec![None; letters.len()];
    for (index, pair) in letters.windows(2).enumerate() {
        let [a, b] = pair else { unreachable!() };
        if a.vertical || b.vertical || !cjk(a) || !cjk(b) {
            continue;
        }
        if !parallel(a.glyph_angle(), b.glyph_angle()) && !punctuation(a) && !punctuation(b) {
            continue;
        }
        // Vertical parentheses may be rotated independently of their upright
        // neighbours. Infer the column from the neighbouring ideograph frame.
        let anchor = if punctuation(a) { b } else { a };
        let inverse = anchor.transform.inverse();
        let delta = inverse * (b.transform * Point::ZERO) - inverse * (a.transform * Point::ZERO);
        if delta.x.abs() <= 150.0 && (-1800.0..=-800.0).contains(&delta.y) {
            let [_, _, c, d, _, _] = anchor.transform.as_coeffs();
            let angle = (-d).atan2(-c);
            columns[index] = Some(angle);
            columns[index + 1] = Some(angle);
        }
    }
    for (letter, column) in letters.iter_mut().zip(columns) {
        if let Some(angle) = column {
            letter.vertical = true;
            letter.direction = Some(angle);
        }
    }
}

impl Flow {
    fn lines(mut self, page_frame: Affine) -> Vec<Line> {
        let rotation = Affine::rotate(-self.angle);
        let vertical = self.vertical;
        // Direction grouping may reorder slightly different source matrices.
        // The pen always consumes the original content-stream sequence.
        self.letters.sort_by_key(|letter| letter.sequence);
        let glyphs = self
            .letters
            .into_iter()
            .map(|letter| {
                let transform = rotation * letter.transform;
                let [_, _, c, d, _, _] = letter.transform.as_coeffs();
                let em = c.hypot(d) * 1000.0;
                let start = transform * Point::ZERO;
                let space = letter.text.as_ref().is_some_and(|text| text.chars().all(char::is_whitespace));
                // Explicit spaces can be narrowed by Tc. Their actual cursor
                // interval is boundary evidence, not the font's nominal width.
                let end = if space {
                    rotation * (letter.transform * Point::ZERO + letter.cursor)
                } else {
                    transform * Point::new(letter.advance.x, letter.advance.y)
                };
                let width = if letter.vertical { 1000.0 } else { letter.advance.x };
                let bounds = if space {
                    Rect::new(start.x.min(end.x), start.y - 0.8 * em, start.x.max(end.x), start.y + 0.2 * em)
                } else {
                    transform.transform_rect_bbox(Rect::new(0.0, -200.0, width, 800.0))
                };
                pen::Glyph { text: letter.text, start, end, em, sequence: letter.sequence, bounds }
            })
            .collect();
        let to_page = page_frame * rotation.inverse();
        let mut lines = arrange(pen::lines(glyphs, vertical));
        for line in &mut lines {
            line.bounds = to_page.transform_rect_bbox(line.bounds);
        }
        lines
    }
}

fn distance(a: Rect, b: Rect) -> f64 {
    let x = (a.x0 - b.x1).max(b.x0 - a.x1).max(0.0);
    let y = (a.y0 - b.y1).max(b.y0 - a.y1).max(0.0);
    x.hypot(y)
}

fn arrange(lines: Vec<Line>) -> Vec<Line> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut paragraphs = std::collections::HashMap::new();
    for line in lines {
        let index = line.paragraph.and_then(|id| paragraphs.get(&id).copied());
        if let Some(index) = index {
            let block: &mut Block = &mut blocks[index];
            block.bounds = block.bounds.union(line.bounds);
            block.lines.push(line);
        } else {
            if let Some(id) = line.paragraph {
                paragraphs.insert(id, blocks.len());
            }
            blocks.push(Block { bounds: line.bounds, em: line.em, lines: vec![line] });
        }
    }
    // Whitespace between lines of one evidenced paragraph must not interleave
    // that paragraph with an adjacent card or column. XY still orders complete
    // lines; the paragraph envelope simply keeps those lines together.
    let mut pending = vec![blocks];
    let mut ordered = Vec::new();
    while let Some(mut region) = pending.pop() {
        if region.is_empty() {
            continue;
        }
        let em = region.iter().map(|line| line.em).sum::<f64>() / region.len() as f64;
        // A short footer can leave the column gutter open below both columns.
        // Prefer the larger separating strip so that the shared space above
        // that footer separates it before the remaining body is split in two.
        let vertical = gap(&region, true).filter(|(_, width)| *width > em);
        let horizontal = gap(&region, false);
        let cut = match (vertical, horizontal) {
            (Some((x, width)), Some((y, height))) => Some(if width > height { (true, x) } else { (false, y) }),
            (Some((x, _)), None) => Some((true, x)),
            (None, Some((y, _))) => Some((false, y)),
            (None, None) => None,
        };
        if let Some((vertical, at)) = cut {
            let (first, second): (Vec<_>, Vec<_>) =
                region.into_iter().partition(|line| if vertical { line.bounds.x0 < at } else { line.bounds.y0 < at });
            pending.push(second);
            pending.push(first);
        } else {
            region.sort_by(|a, b| a.bounds.y0.total_cmp(&b.bounds.y0).then(a.bounds.x0.total_cmp(&b.bounds.x0)));
            for mut block in region {
                block
                    .lines
                    .sort_by(|a, b| a.bounds.y0.total_cmp(&b.bounds.y0).then(a.bounds.x0.total_cmp(&b.bounds.x0)));
                ordered.extend(block.lines);
            }
        }
    }
    ordered
}

struct Block {
    lines: Vec<Line>,
    bounds: Rect,
    em: f64,
}

fn gap(lines: &[Block], vertical: bool) -> Option<(f64, f64)> {
    let mut intervals: Vec<_> = lines
        .iter()
        .map(|line| if vertical { (line.bounds.x0, line.bounds.x1) } else { (line.bounds.y0, line.bounds.y1) })
        .collect();
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
