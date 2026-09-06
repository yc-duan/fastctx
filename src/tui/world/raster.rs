//! A dot-level raster for terminal diagrams: 2×4 coloured dots per cell, composed into the best
//! glyph each cell can show, with text laid on top.
//!
//! A terminal has no pixels, only cells; the finest thing a cell can show is a Braille pattern
//! (two columns by four rows of dots), and a cell has exactly one foreground and one background
//! colour. This module keeps a dot grid at that resolution and decides per cell how to present
//! it: a cell whose dots form a full, upper, lower, left or right half is drawn as the matching
//! block element (`█ ▀ ▄ ▌ ▐`) so filled panels are solid and can carry a second colour in the
//! other half; any other pattern (a diagonal, a curve, a thin frame) becomes a Braille glyph in
//! the colour of its dots. Panels therefore look like surfaces and connectors look like lines,
//! on one canvas, in one pass.
//!
//! Terminals whose fonts lack Braille (`WT_SESSION`-less conhost, the Linux VT) get the same
//! drawing composed from block elements only; those five glyphs have existed since code page
//! 437. `FASTCTX_TUI_GLYPHS=braille|blocks` overrides the detection.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use std::sync::OnceLock;

const FULL: &str = "█";
const UPPER: &str = "▀";
const LOWER: &str = "▄";
const LEFT: &str = "▌";
const RIGHT: &str = "▐";

/// Which partial-cell glyphs the terminal can be trusted to render.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Glyphs {
    /// Block elements plus Braille patterns: thin lines keep their dot resolution.
    Braille,
    /// Block elements only: every partial cell rounds to the nearest half block.
    Blocks,
}

fn detect_glyphs(mut value: impl FnMut(&str) -> Option<String>) -> Glyphs {
    match value("FASTCTX_TUI_GLYPHS").as_deref() {
        Some("braille") => return Glyphs::Braille,
        Some("blocks") => return Glyphs::Blocks,
        _ => {}
    }
    let term = value("TERM").unwrap_or_default().to_ascii_lowercase();
    if term == "dumb" || term == "linux" {
        return Glyphs::Blocks;
    }
    if cfg!(windows) {
        // Windows Terminal and editors fall back to a symbol font per glyph; the classic console
        // host draws one font and shows tofu for anything outside it.
        let modern = value("WT_SESSION").is_some()
            || value("WEZTERM_EXECUTABLE").is_some()
            || value("TERM_PROGRAM").is_some()
            || value("ConEmuANSI").is_some();
        return if modern {
            Glyphs::Braille
        } else {
            Glyphs::Blocks
        };
    }
    Glyphs::Braille
}

pub(super) fn glyphs() -> Glyphs {
    static DETECTED: OnceLock<Glyphs> = OnceLock::new();
    *DETECTED.get_or_init(|| {
        detect_glyphs(|name| {
            std::env::var_os(name).map(|value| value.to_string_lossy().into_owned())
        })
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Stroke {
    Solid,
    /// Four dots on, four off: a dash is one cell tall and two columns long.
    Dashed,
}

/// A point in dot coordinates; x runs two per column, y four per row, origin at the top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Dot {
    pub(super) x: f64,
    pub(super) y: f64,
}

impl Dot {
    pub(super) const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

struct Label {
    column: u16,
    row: u16,
    text: String,
    style: Style,
    max_width: u16,
}

pub(super) struct Surface {
    columns: u16,
    rows: u16,
    glyphs: Glyphs,
    dots: Vec<Option<Color>>,
    labels: Vec<Label>,
}

impl Surface {
    pub(super) fn new(columns: u16, rows: u16, glyphs: Glyphs) -> Self {
        Self {
            columns,
            rows,
            glyphs,
            dots: vec![None; usize::from(columns) * 2 * usize::from(rows) * 4],
            labels: Vec::new(),
        }
    }

    /// Width in dots: two per column.
    pub(super) fn width(&self) -> i32 {
        i32::from(self.columns) * 2
    }

    /// Height in dots: four per row.
    pub(super) fn height(&self) -> i32 {
        i32::from(self.rows) * 4
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        (x >= 0 && y >= 0 && x < self.width() && y < self.height())
            .then(|| y as usize * usize::from(self.columns) * 2 + x as usize)
    }

    pub(super) fn set(&mut self, x: i32, y: i32, color: Color) {
        if let Some(index) = self.index(x, y) {
            self.dots[index] = Some(color);
        }
    }

    fn clear(&mut self, x: i32, y: i32) {
        if let Some(index) = self.index(x, y) {
            self.dots[index] = None;
        }
    }

    pub(super) fn fill(&mut self, x: i32, y: i32, width: i32, height: i32, color: Color) {
        for py in y..y + height {
            for px in x..x + width {
                self.set(px, py, color);
            }
        }
    }

    /// One-dot outline of a rectangle; thin, so it renders as Braille where available.
    pub(super) fn frame(&mut self, x: i32, y: i32, width: i32, height: i32, color: Color) {
        if width <= 0 || height <= 0 {
            return;
        }
        for px in x..x + width {
            self.set(px, y, color);
            self.set(px, y + height - 1, color);
        }
        for py in y..y + height {
            self.set(x, py, color);
            self.set(x + width - 1, py, color);
        }
    }

    /// A filled panel whose corners are chamfered by one half cell, which reads as rounded at
    /// cell scale. Align `x` and `width` to even dots and `y` and `height` to multiples of four so
    /// the body composes into solid cells; with `fill` absent only the outline is drawn.
    pub(super) fn panel(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        fill: Option<Color>,
        outline: Option<Color>,
    ) {
        if width < 4 || height < 4 {
            return;
        }
        if let Some(color) = fill {
            self.fill(x, y, width, height, color);
            for dx in 0..2 {
                for dy in 0..2 {
                    self.clear(x + dx, y + dy);
                    self.clear(x + width - 1 - dx, y + dy);
                    self.clear(x + dx, y + height - 1 - dy);
                    self.clear(x + width - 1 - dx, y + height - 1 - dy);
                }
            }
        }
        if let Some(color) = outline {
            self.frame(x, y, width, height, color);
        }
    }

    /// Bresenham line between two dots.
    pub(super) fn line(&mut self, from: Dot, to: Dot, color: Color, stroke: Stroke) {
        self.line_with(from, to, stroke, 0, |_| color);
    }

    /// A line whose colour changes along its length, from `start` at `from` to `end` at `to`.
    /// True-colour terminals get the blend; every other palette holds `start`.
    pub(super) fn gradient(
        &mut self,
        from: Dot,
        to: Dot,
        start: Color,
        end: Color,
        stroke: Stroke,
    ) {
        let length = (to.x - from.x).hypot(to.y - from.y).max(1.0);
        self.line_with(from, to, stroke, 0, |travelled| {
            blend(start, end, (travelled / length).clamp(0.0, 1.0))
        });
    }

    fn line_with(
        &mut self,
        from: Dot,
        to: Dot,
        stroke: Stroke,
        first_step: i32,
        color: impl Fn(f64) -> Color,
    ) -> i32 {
        let (x1, y1) = (from.x.round() as i32, from.y.round() as i32);
        let (x2, y2) = (to.x.round() as i32, to.y.round() as i32);
        let dx = (x2 - x1).abs();
        let dy = -(y2 - y1).abs();
        let sx = if x1 < x2 { 1 } else { -1 };
        let sy = if y1 < y2 { 1 } else { -1 };
        let mut error = dx + dy;
        let (mut x, mut y) = (x1, y1);
        let mut step = first_step;
        loop {
            let visible = match stroke {
                Stroke::Solid => true,
                Stroke::Dashed => (step / 4) % 2 == 0,
            };
            if visible {
                let travelled = f64::from(x - x1).hypot(f64::from(y - y1));
                self.set(x, y, color(travelled));
            }
            if x == x2 && y == y2 {
                break;
            }
            let twice = 2 * error;
            if twice >= dy {
                error += dy;
                x += sx;
            }
            if twice <= dx {
                error += dx;
                y += sy;
            }
            step += 1;
        }
        step
    }

    /// A quadratic Bézier curve, sampled densely enough that consecutive samples touch.
    pub(super) fn curve(&mut self, from: Dot, control: Dot, to: Dot, color: Color, stroke: Stroke) {
        let extent = (control.x - from.x).abs()
            + (to.x - control.x).abs()
            + (control.y - from.y).abs()
            + (to.y - control.y).abs();
        let samples = (extent.ceil() as usize).max(2);
        let mut previous = from;
        let mut step = 0;
        for index in 1..=samples {
            let t = index as f64 / samples as f64;
            let u = 1.0 - t;
            let point = Dot::new(
                u * u * from.x + 2.0 * u * t * control.x + t * t * to.x,
                u * u * from.y + 2.0 * u * t * control.y + t * t * to.y,
            );
            step = self.line_with(previous, point, stroke, step, |_| color);
            previous = point;
        }
    }

    /// Queue text for a cell position; drawn after every dot so it always sits on top. Style
    /// fields left unset inherit from the composed cell underneath, so text inside a filled
    /// panel takes the panel's colour as its background without the caller repeating it.
    pub(super) fn label(
        &mut self,
        column: i32,
        row: i32,
        text: impl Into<String>,
        style: Style,
        max_width: i32,
    ) {
        if column < 0 || row < 0 || column >= i32::from(self.columns) || row >= i32::from(self.rows)
        {
            return;
        }
        let available = (i32::from(self.columns) - column).min(max_width.max(0));
        if available == 0 {
            return;
        }
        self.labels.push(Label {
            column: column as u16,
            row: row as u16,
            text: text.into(),
            style,
            max_width: available as u16,
        });
    }

    pub(super) fn render(&self, area: Rect, buf: &mut Buffer) {
        let columns = usize::from(self.columns.min(area.width));
        let rows = usize::from(self.rows.min(area.height));
        let stride = usize::from(self.columns) * 2;
        for row in 0..rows {
            for column in 0..columns {
                let mut cell_dots = [None; 8];
                for (index, slot) in cell_dots.iter_mut().enumerate() {
                    let (dx, dy) = (index % 2, index / 2);
                    *slot = self.dots[(row * 4 + dy) * stride + column * 2 + dx];
                }
                let Some(composed) = compose(&cell_dots, self.glyphs) else {
                    continue;
                };
                if let Some(cell) = buf.cell_mut((area.x + column as u16, area.y + row as u16)) {
                    cell.set_symbol(composed.symbol);
                    cell.set_fg(composed.fg);
                    if let Some(bg) = composed.bg {
                        cell.set_bg(bg);
                    }
                }
            }
        }
        for label in &self.labels {
            if label.row >= area.height || label.column >= area.width {
                continue;
            }
            let max_width = label.max_width.min(area.width - label.column);
            buf.set_stringn(
                area.x + label.column,
                area.y + label.row,
                &label.text,
                usize::from(max_width),
                label.style,
            );
        }
    }
}

struct Composed {
    symbol: &'static str,
    fg: Color,
    bg: Option<Color>,
}

/// Dots are indexed row-major: 0 1 / 2 3 / 4 5 / 6 7.
fn compose(dots: &[Option<Color>; 8], glyphs: Glyphs) -> Option<Composed> {
    let set = dots.iter().filter(|dot| dot.is_some()).count();
    if set == 0 {
        return None;
    }
    let top = &dots[0..4];
    let bottom = &dots[4..8];
    let uniform = |part: &[Option<Color>]| -> Option<Color> {
        let first = part[0]?;
        part.iter().all(|dot| *dot == Some(first)).then_some(first)
    };
    let empty = |part: &[Option<Color>]| part.iter().all(Option::is_none);
    match (uniform(top), uniform(bottom)) {
        (Some(upper), Some(lower)) if upper == lower => {
            return Some(Composed {
                symbol: FULL,
                fg: upper,
                bg: Some(lower),
            });
        }
        (Some(upper), Some(lower)) => {
            return Some(Composed {
                symbol: UPPER,
                fg: upper,
                bg: Some(lower),
            });
        }
        (Some(upper), None) if empty(bottom) => {
            return Some(Composed {
                symbol: UPPER,
                fg: upper,
                bg: None,
            });
        }
        (None, Some(lower)) if empty(top) => {
            return Some(Composed {
                symbol: LOWER,
                fg: lower,
                bg: None,
            });
        }
        _ => {}
    }
    let left = [dots[0], dots[2], dots[4], dots[6]];
    let right = [dots[1], dots[3], dots[5], dots[7]];
    match (uniform(&left), uniform(&right)) {
        (Some(color), None) if empty(&right) => {
            return Some(Composed {
                symbol: LEFT,
                fg: color,
                bg: None,
            });
        }
        (None, Some(color)) if empty(&left) => {
            return Some(Composed {
                symbol: RIGHT,
                fg: color,
                bg: None,
            });
        }
        _ => {}
    }
    let fg = majority(dots);
    match glyphs {
        Glyphs::Braille => Some(Composed {
            symbol: braille(dots),
            fg,
            bg: None,
        }),
        Glyphs::Blocks => {
            let count = |part: &[Option<Color>]| part.iter().filter(|dot| dot.is_some()).count();
            let (t, b, l, r) = (count(top), count(bottom), count(&left), count(&right));
            let symbol = if set >= 6 {
                FULL
            } else if t >= b && t >= l && t >= r {
                UPPER
            } else if b >= l && b >= r {
                LOWER
            } else if l >= r {
                LEFT
            } else {
                RIGHT
            };
            Some(Composed {
                symbol,
                fg,
                bg: None,
            })
        }
    }
}

fn majority(dots: &[Option<Color>; 8]) -> Color {
    let mut best: Option<(Color, usize)> = None;
    for candidate in dots.iter().flatten() {
        let votes = dots.iter().filter(|dot| **dot == Some(*candidate)).count();
        if best.is_none_or(|(_, current)| votes > current) {
            best = Some((*candidate, votes));
        }
    }
    best.map(|(color, _)| color).unwrap_or(Color::Reset)
}

/// Braille dot bits: column 0 rows 0..3 are 0x01 0x02 0x04 0x40, column 1 rows 0..3 are
/// 0x08 0x10 0x20 0x80. The 256 glyphs are contiguous from U+2800.
fn braille(dots: &[Option<Color>; 8]) -> &'static str {
    const BITS: [u32; 8] = [0x01, 0x08, 0x02, 0x10, 0x04, 0x20, 0x40, 0x80];
    static GLYPHS: OnceLock<Vec<String>> = OnceLock::new();
    let table = GLYPHS.get_or_init(|| {
        (0u32..256)
            .map(|bits| char::from_u32(0x2800 + bits).unwrap_or(' ').to_string())
            .collect()
    });
    let mut bits = 0;
    for (index, dot) in dots.iter().enumerate() {
        if dot.is_some() {
            bits |= BITS[index];
        }
    }
    table[bits as usize].as_str()
}

fn blend(start: Color, end: Color, t: f64) -> Color {
    match (start, end) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let mix =
                |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as u8;
            Color::Rgb(mix(r1, r2), mix(g1, g2), mix(b1, b2))
        }
        _ => start,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(surface: &Surface) -> Buffer {
        let area = Rect::new(0, 0, surface.columns, surface.rows);
        let mut buf = Buffer::empty(area);
        surface.render(area, &mut buf);
        buf
    }

    #[test]
    fn uniform_halves_become_block_elements_and_partials_become_braille() {
        let mut surface = Surface::new(4, 1, Glyphs::Braille);
        surface.fill(0, 0, 2, 4, Color::Red);
        surface.fill(2, 0, 2, 2, Color::Blue);
        surface.fill(4, 2, 2, 2, Color::Green);
        surface.set(6, 0, Color::Yellow);
        surface.set(7, 3, Color::Yellow);
        let buf = render(&surface);
        assert_eq!(buf[(0, 0)].symbol(), FULL);
        assert_eq!(buf[(0, 0)].bg, Color::Red);
        assert_eq!(buf[(1, 0)].symbol(), UPPER);
        assert_eq!(buf[(1, 0)].fg, Color::Blue);
        assert_eq!(buf[(2, 0)].symbol(), LOWER);
        assert_eq!(buf[(3, 0)].symbol(), "⢁");
        assert_eq!(buf[(3, 0)].fg, Color::Yellow);
    }

    #[test]
    fn block_only_terminals_round_partials_to_the_nearest_half() {
        let mut surface = Surface::new(2, 1, Glyphs::Blocks);
        surface.set(0, 0, Color::Yellow);
        surface.set(1, 1, Color::Yellow);
        surface.set(2, 0, Color::Cyan);
        surface.set(2, 1, Color::Cyan);
        surface.set(2, 2, Color::Cyan);
        let buf = render(&surface);
        assert_eq!(buf[(0, 0)].symbol(), UPPER);
        assert_eq!(buf[(1, 0)].symbol(), LEFT);
    }

    #[test]
    fn two_colours_in_one_cell_keep_both_through_foreground_and_background() {
        let mut surface = Surface::new(1, 1, Glyphs::Braille);
        surface.fill(0, 0, 2, 2, Color::Blue);
        surface.fill(0, 2, 2, 2, Color::Green);
        let buf = render(&surface);
        assert_eq!(buf[(0, 0)].symbol(), UPPER);
        assert_eq!(buf[(0, 0)].fg, Color::Blue);
        assert_eq!(buf[(0, 0)].bg, Color::Green);
    }

    #[test]
    fn labels_inherit_the_panel_ground_and_clip_at_their_width() {
        let mut surface = Surface::new(8, 2, Glyphs::Braille);
        surface.panel(0, 0, 16, 8, Some(Color::Blue), None);
        surface.label(1, 0, "中文 wide", Style::default().fg(Color::White), 6);
        let buf = render(&surface);
        assert_eq!(buf[(1, 0)].symbol(), "中");
        assert_eq!(buf[(1, 0)].bg, Color::Blue);
        assert_eq!(buf[(1, 0)].fg, Color::White);
        assert_eq!(buf[(6, 0)].symbol(), "w");
        assert_eq!(
            buf[(7, 0)].symbol(),
            LOWER,
            "the chamfered corner is a lower half block"
        );
        assert_eq!(buf[(0, 1)].symbol(), UPPER);
    }

    #[test]
    fn curves_and_dashes_stay_within_the_surface() {
        let mut surface = Surface::new(10, 4, Glyphs::Braille);
        surface.curve(
            Dot::new(-5.0, -5.0),
            Dot::new(30.0, 0.0),
            Dot::new(19.0, 15.0),
            Color::Red,
            Stroke::Dashed,
        );
        surface.gradient(
            Dot::new(0.0, 15.0),
            Dot::new(19.0, 0.0),
            Color::Rgb(0, 0, 0),
            Color::Rgb(255, 255, 255),
            Stroke::Solid,
        );
        let buf = render(&surface);
        assert!(buf.content.iter().any(|cell| cell.symbol() != " "));
        assert_ne!(
            buf[(0, 3)].fg,
            buf[(9, 0)].fg,
            "the gradient changes along the line"
        );
    }

    #[test]
    fn glyph_detection_prefers_blocks_on_the_classic_windows_console() {
        let detect = |vars: &[(&str, &str)]| {
            detect_glyphs(|name| {
                vars.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_string())
            })
        };
        assert_eq!(detect(&[("FASTCTX_TUI_GLYPHS", "blocks")]), Glyphs::Blocks);
        assert_eq!(detect(&[("TERM", "linux")]), Glyphs::Blocks);
        assert_eq!(
            detect(&[("WT_SESSION", "x"), ("TERM", "xterm-256color")]),
            Glyphs::Braille
        );
        if cfg!(windows) {
            assert_eq!(detect(&[]), Glyphs::Blocks);
        } else {
            assert_eq!(detect(&[("TERM", "xterm")]), Glyphs::Braille);
        }
    }
}
