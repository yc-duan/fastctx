//! Modified by fastctx: read-only glyph extraction, including annotation appearances.

use super::*;
use crate::content::graphics_state::Matrix;
use crate::content::operators::Operator;
use crate::extractors::{TextExtractor, text::RawTextGlyph};

impl PdfDocument {
    /// Open a read-only file without populating source_bytes for editing APIs.
    pub fn open_readonly(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_from_reader(PdfReader::File(BufReader::new(std::fs::File::open(path)?)))
    }

    /// Extract glyph clusters in content-stream order, without layout decisions.
    pub fn extract_raw_glyphs(&self, page_index: usize) -> Result<Vec<RawTextGlyph>> {
        let page = self.get_page(page_index)?;
        let dict = page.as_dict().ok_or_else(|| invalid("Page is not a dictionary"))?;
        let content = self.cached_page_content(page_index)?;
        let mut extractor = TextExtractor::new();
        extractor.set_page_index(page_index as u32);
        extractor.set_document(self);
        if let Some(resources) = dict.get("Resources") {
            extractor.set_resources(resources.clone());
            self.load_fonts(resources, &mut extractor)?;
        }
        let mut glyphs = extractor.extract_raw_owned(&content)?;
        if let Some(annotations) = dict.get("Annots").filter(|object| !matches!(object, Object::Null)) {
            let annotations = self.raw_dereference(annotations)?;
            for annotation in annotations.as_array().ok_or_else(|| invalid("Annots is not an array"))? {
                glyphs.extend(self.raw_appearance(annotation, page_index)?);
            }
        }
        Ok(glyphs)
    }

    /// Release page working data after a read, retaining metadata and fonts.
    ///
    /// Call only between extractions on this document. Font-set entries include
    /// extractor-local inherited bindings; they must not survive into another
    /// page's font environment. Individual parsed fonts remain reusable.
    pub fn release_page_data(&self) {
        self.page_content_cache.lock_or_recover().clear();
        self.xobject_stream_cache.lock_or_recover().clear();
        self.xobject_stream_cache_bytes.store(0, Ordering::Relaxed);
        self.font_set_cache.lock_or_recover().clear();
        self.font_fingerprint_cache.lock_or_recover().clear();
        self.font_name_set_cache.lock_or_recover().clear();
        self.object_cache.lock_or_recover().release_streams();
    }

    fn raw_dereference(&self, object: &Object) -> Result<Object> {
        match object.as_reference() {
            Some(reference) => self.load_object(reference),
            None => Ok(object.clone()),
        }
    }

    fn raw_appearance(&self, annotation: &Object, page: usize) -> Result<Vec<RawTextGlyph>> {
        let annotation = self.raw_dereference(annotation)?;
        let dict = annotation.as_dict().ok_or_else(|| invalid("annotation is not a dictionary"))?;
        let flags = dict.get("F").and_then(Object::as_integer).unwrap_or(0);
        if flags & 35 != 0 {
            return Ok(Vec::new());
        }
        let Some(ap) = dict.get("AP").filter(|object| !matches!(object, Object::Null)) else {
            return Ok(Vec::new());
        };
        let ap = self.raw_dereference(ap)?;
        let Some(normal) = ap.as_dict().and_then(|ap| ap.get("N")).filter(|object| !matches!(object, Object::Null))
        else {
            return Ok(Vec::new());
        };
        let mut stream_ref = normal.clone();
        let mut stream = self.raw_dereference(normal)?;
        if let Object::Dictionary(states) = &stream {
            let Some(state) = dict.get("AS").and_then(Object::as_name) else {
                return Ok(Vec::new());
            };
            let Some(selected) = states.get(state) else {
                return Ok(Vec::new());
            };
            stream_ref = selected.clone();
            stream = self.raw_dereference(&stream_ref)?;
        }
        let Object::Stream { dict: appearance, .. } = &stream else {
            return Err(invalid("normal appearance is not a stream"));
        };
        let data = match stream_ref.as_reference() {
            Some(reference) => self.decode_stream_with_encryption(&stream, reference)?,
            None => stream.decode_stream_data()?,
        };
        let rect = numbers::<4>(dict.get("Rect"))?;
        let bbox = numbers::<4>(appearance.get("BBox"))?;
        let [a, b, c, d, e, f] = if appearance.contains_key("Matrix") {
            numbers::<6>(appearance.get("Matrix"))?
        } else {
            [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
        };
        let matrix = Matrix { a, b, c, d, e, f };
        let corners = [(bbox[0], bbox[1]), (bbox[2], bbox[1]), (bbox[0], bbox[3]), (bbox[2], bbox[3])]
            .map(|(x, y)| matrix.transform_point(x, y));
        let left = corners.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
        let right = corners.iter().map(|p| p.x).fold(f32::NEG_INFINITY, f32::max);
        let bottom = corners.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
        let top = corners.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
        if right == left || top == bottom {
            return Err(invalid("annotation appearance has an empty BBox"));
        }
        // ISO 32000-1 12.5.5 maps the transformed appearance box onto Rect.
        let sx = (rect[2] - rect[0]).abs() / (right - left);
        let sy = (rect[3] - rect[1]).abs() / (top - bottom);
        let fit = Matrix {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            e: rect[0].min(rect[2]) - sx * left,
            f: rect[1].min(rect[3]) - sy * bottom,
        };
        let matrix = matrix.multiply(&fit);
        let mut extractor = TextExtractor::new();
        extractor.set_page_index(page as u32);
        extractor.set_document(self);
        if let Some(resources) = appearance.get("Resources") {
            let resources = self.raw_dereference(resources)?;
            extractor.set_resources(resources.clone());
            self.load_fonts(&resources, &mut extractor)?;
        }
        extractor.execute_operator_public(Operator::Cm {
            a: matrix.a,
            b: matrix.b,
            c: matrix.c,
            d: matrix.d,
            e: matrix.e,
            f: matrix.f,
        })?;
        extractor.extract_raw_owned(&data)
    }
}

fn invalid(reason: &str) -> Error {
    Error::ParseError { offset: 0, reason: reason.into() }
}

fn numbers<const N: usize>(object: Option<&Object>) -> Result<[f32; N]> {
    let array = object.and_then(Object::as_array).ok_or_else(|| invalid("missing appearance rectangle or matrix"))?;
    if array.len() != N {
        return Err(invalid("wrong appearance rectangle or matrix length"));
    }
    let mut result = [0.0; N];
    for (index, object) in array.iter().enumerate() {
        let number = match object {
            Object::Integer(n) => *n as f32,
            Object::Real(n) => *n as f32,
            _ => return Err(invalid("non-numeric appearance coordinate")),
        };
        if !number.is_finite() {
            return Err(invalid("non-finite appearance coordinate"));
        }
        result[index] = number;
    }
    Ok(result)
}
