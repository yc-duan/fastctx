//! Modified by fastctx: original glyph events for downstream text layout.

use super::*;

/// One original glyph code, with its complete Unicode mapping kept together.
#[derive(Debug)]
pub struct RawTextGlyph {
    /// Unicode from font resources; None means no mapping is available.
    pub text: Option<String>,
    /// Resolved font name, for diagnostics.
    pub font_name: String,
    /// Font identity within this extractor, for overprint comparisons.
    pub font_id: usize,
    /// Original PDF character code.
    pub char_code: u32,
    /// Normalized 1000-unit glyph frame at the text cursor, including text rise.
    pub matrix: [f32; 6],
    /// Font-defined advance in that frame, before Tc, Tw and TJ adjustments.
    pub advance: [f32; 2],
    /// Actual advance in page space, including Tc and Tw, but not TJ offsets.
    pub rendered_advance: [f32; 2],
    /// Glyph origin in page space, including W2 origin displacement.
    pub glyph_origin: [f32; 2],
    /// Typographic quadrilateral in page space, from font ascent and descent.
    pub corners: [[f32; 2]; 4],
    /// Whether WMode is 1.
    pub vertical: bool,
    /// Whether this glyph uses a painting text-rendering mode.
    pub visible: bool,
    /// Tagged PDF marked-content identifier.
    pub mcid: Option<u32>,
}

impl TextExtractor<'_> {
    /// Interpret every glyph in paint order; do not sort, deduplicate or add spaces.
    pub fn extract_raw_owned(&mut self, content: &[u8]) -> Result<Vec<RawTextGlyph>> {
        self.extract_spans = false;
        self.chars.clear();
        self.spans.clear();
        self.raw_glyphs = Some(Vec::new());
        self.placed_pdf_keep = true;
        self.run_content_stream(content)?;
        Ok(self.raw_glyphs.take().unwrap())
    }

    pub(super) fn show_raw_text(&mut self, text: &[u8]) -> Result<()> {
        // The content parser already owns the complete string. Raw extraction
        // must not apply the legacy character API's string/ligature truncation.
        let font = self.cached_current_font.clone();
        for (code, nbytes) in TextCharIter::new(text, font.as_deref()) {
            let state = self.state_stack.current();
            let fs = state.font_size;
            let hs = state.horizontal_scaling / 100.0;
            let vertical = state.text_wmode != 0;
            let fm = font.as_ref().map(|font| font.font_matrix_a).unwrap_or(0.001);
            let width = font
                .as_ref()
                .map(|font| if vertical { font.get_vertical_metrics(code).w1y } else { font.get_glyph_width(code) })
                .unwrap_or(if vertical { -1000.0 } else { 500.0 });
            let advance = width * fm * 1000.0;
            let word_space = if code == 32 && nbytes == 1 { state.word_space } else { 0.0 };
            let cursor = (width * fm * fs + state.char_space + word_space) * if vertical { 1.0 } else { hs };
            let unicode = font
                .as_ref()
                .and_then(|font| {
                    (font.best_mapping_provenance() != crate::fonts::MappingProvenance::Fallback)
                        .then(|| font.char_to_unicode(u32::from(code)))
                        .flatten()
                })
                .filter(|text| !text.is_empty() && text.chars().any(|c| c != '\u{fffd}'));
            let glyph = self.raw_glyph(unicode, u32::from(code), advance, cursor);
            if !self.is_content_suppressed() {
                self.raw_glyphs.as_mut().unwrap().push(glyph);
            }
            self.state_stack.current_mut().advance_text_matrix(cursor);
        }
        Ok(())
    }

    fn raw_glyph(&self, text: Option<String>, code: u32, advance: f32, cursor: f32) -> RawTextGlyph {
        let state = self.state_stack.current();
        // Matrix::multiply uses row-vector composition despite its doc comment:
        // apply the text matrix first, then the current transformation matrix.
        let combined = state.text_matrix.multiply(&state.ctm);
        let origin = combined.transform_point(0.0, state.text_rise);
        let scale = state.font_size / 1000.0;
        let hs = state.horizontal_scaling / 100.0;
        let vertical = state.text_wmode != 0;
        let font = self.cached_current_font.as_ref();
        let glyph_origin = if vertical {
            let metrics = font
                .map(|font| font.get_vertical_metrics(code as u16))
                .unwrap_or(crate::fonts::VerticalMetrics::SPEC_DEFAULT);
            combined.transform_point(-metrics.v_x * scale * hs, state.text_rise - metrics.v_y * scale)
        } else {
            origin
        };
        let width =
            font.map(|font| font.get_glyph_width(code as u16) * font.font_matrix_a * 1000.0).unwrap_or(advance.abs());
        let descent = font.map_or(-0.35, |font| font.descent) * 1000.0;
        let ascent = font.map_or(0.95, |font| font.ascent) * 1000.0;
        let corners = [[0.0, descent], [width, descent], [width, ascent], [0.0, ascent]].map(|[x, y]| {
            [
                glyph_origin.x + combined.a * scale * hs * x + combined.c * scale * y,
                glyph_origin.y + combined.b * scale * hs * x + combined.d * scale * y,
            ]
        });
        RawTextGlyph {
            text,
            char_code: code,
            font_name: font.map(|font| font.base_font.clone()).unwrap_or_default(),
            font_id: font.map(|font| Arc::as_ptr(font) as usize).unwrap_or(0),
            matrix: [
                combined.a * scale * hs,
                combined.b * scale * hs,
                combined.c * scale,
                combined.d * scale,
                origin.x,
                origin.y,
            ],
            advance: if vertical { [0.0, advance] } else { [advance, 0.0] },
            rendered_advance: if vertical {
                [combined.c * cursor, combined.d * cursor]
            } else {
                [combined.a * cursor, combined.b * cursor]
            },
            glyph_origin: [glyph_origin.x, glyph_origin.y],
            corners,
            vertical,
            visible: state.render_mode != 3 && state.render_mode != 7,
            mcid: self.current_mcid,
        }
    }

    pub(super) fn clip_raw_glyphs(&mut self, first: usize, bounds: [f32; 4], matrix: Matrix) {
        let Some(glyphs) = &mut self.raw_glyphs else { return };
        let [x0, y0, x1, y1] = bounds;
        let clip = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].map(|[x, y]| {
            let point = matrix.transform_point(x, y);
            [point.x, point.y]
        });
        let mut index = 0;
        glyphs.retain(|glyph| {
            let keep = index < first || intersects(&glyph.corners, &clip);
            index += 1;
            keep
        });
    }

    pub(super) fn show_raw_actual(&mut self, actual: &str, strings: &[TextElement]) -> Result<()> {
        let mut glyph = self.raw_glyph(Some(actual.to_owned()), 0, 0.0, 0.0);
        let start = self.state_stack.current().text_matrix.multiply(&self.state_stack.current().ctm);
        for element in strings {
            match element {
                TextElement::String(text) => {
                    self.advance_position_for_string(text)?;
                }
                TextElement::Offset(offset) => {
                    self.advance_position_for_offset(*offset)?;
                }
            }
        }
        let end = self.state_stack.current().text_matrix.multiply(&self.state_stack.current().ctm);
        let dx = end.e - start.e;
        let dy = end.f - start.f;
        let axis = if glyph.vertical { [glyph.matrix[2], glyph.matrix[3]] } else { [glyph.matrix[0], glyph.matrix[1]] };
        let norm = axis[0] * axis[0] + axis[1] * axis[1];
        let advance = if norm == 0.0 { 0.0 } else { (dx * axis[0] + dy * axis[1]) / norm };
        glyph.advance = if glyph.vertical { [0.0, advance] } else { [advance, 0.0] };
        glyph.rendered_advance = [dx, dy];
        if !self.is_content_suppressed() {
            self.raw_glyphs.as_mut().unwrap().push(glyph);
        }
        Ok(())
    }
}

// The separating-axis test preserves partially clipped letters and works for
// rotated/sheared Forms. Every enclosing Form applies its own intersection.
fn intersects(a: &[[f32; 2]; 4], b: &[[f32; 2]; 4]) -> bool {
    for polygon in [a, b] {
        for edge in 0..4 {
            let p = polygon[edge];
            let q = polygon[(edge + 1) % 4];
            let axis = [f64::from(q[1]) - f64::from(p[1]), f64::from(p[0]) - f64::from(q[0])];
            let project = |corners: &[[f32; 2]; 4]| {
                corners
                    .iter()
                    .map(|p| f64::from(p[0]) * axis[0] + f64::from(p[1]) * axis[1])
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), value| (lo.min(value), hi.max(value)))
            };
            let (al, ah) = project(a);
            let (bl, bh) = project(b);
            // Both endpoints originated as f32 coordinates.
            let rounding = (al.abs() + ah.abs() + bl.abs() + bh.abs()) * f64::from(f32::EPSILON);
            if ah < bl - rounding || bh < al - rounding {
                return false;
            }
        }
    }
    true
}
