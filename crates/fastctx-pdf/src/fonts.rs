//! Select installed substitute fonts against the characters actually painted.

use hayro::hayro_interpret::font::{FallbackFontQuery, FontData, FontQuery, Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::{BfString, CidFamily};
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps, InterpreterCache,
    InterpreterSettings, SoftMask, TransformExt, interpret_page,
};
use hayro::hayro_syntax::page::Page;
use hayro::kurbo::{BezPath, Rect};
use skrifa::{FontRef, MetadataProvider};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

pub(super) type Database = OnceLock<fontdb::Database>;

pub(super) fn prepare(
    page: &Page<'_>,
    settings: &InterpreterSettings,
    database: &Database,
) -> (InterpreterSettings, Vec<String>) {
    let queries = Arc::new(Mutex::new(HashMap::new()));
    let sink = queries.clone();
    let default = settings.font_resolver.clone();
    let mut probe_settings = settings.clone();
    probe_settings.font_resolver = Arc::new(move |query| {
        if let FontQuery::Fallback(query) = query {
            sink.lock().unwrap().insert(name(query), query.clone());
        }
        default(query)
    });
    // Probe glyph mappings without decoding images or drawing their pixels.
    // The second pass sees the chosen local faces; embedded fonts never query
    // the resolver and therefore retain their original programs and metrics.
    let (width, height) = page.render_dimensions();
    let cache = InterpreterCache::new();
    let mut context = Context::new(
        page.initial_transform(true).to_kurbo(),
        Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
        &cache,
        page.xref(),
        probe_settings,
    );
    let mut used = Used::default();
    interpret_page(page, &mut context, &mut used);
    let queries = queries.lock().unwrap();
    if queries.is_empty() {
        return (settings.clone(), Vec::new());
    }
    let database = database.get_or_init(|| {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        database
    });
    let mut selected: HashMap<String, (FontData, u32)> = HashMap::new();
    let mut notes = Vec::new();
    for (name, query) in queries.iter() {
        let chars = used.chars.get(name).cloned().unwrap_or_default();
        if chars.is_empty() {
            continue;
        }
        let requested: Vec<_> = [&query.post_script_name, &query.font_name, &query.font_family]
            .into_iter()
            .flatten()
            .map(|name| normalize(name))
            .collect();
        let region = regional_names(query);
        let fallback = (settings.font_resolver)(&FontQuery::Fallback(query.clone()));
        let fallback_chars =
            fallback.as_ref().map(|(data, index)| coverage(data.as_ref().as_ref(), *index, &chars)).unwrap_or_default();
        let mut best = None;
        for face in database.faces() {
            let covered = database
                .with_face_data(face.id, |data, index| {
                    FontRef::from_index(data, index)
                        .ok()
                        .map(|font| {
                            let cmap = font.charmap();
                            chars.iter().filter(|&&c| cmap.map(c).is_some()).count()
                        })
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            if covered == 0 {
                continue;
            }
            let names: Vec<_> = std::iter::once(&face.post_script_name)
                .chain(face.families.iter().map(|family| &family.0))
                .map(|name| normalize(name))
                .collect();
            let exact = names.iter().any(|name| requested.contains(name));
            let regional = region
                .iter()
                .rev()
                .position(|preferred| names.iter().any(|name| name.contains(preferred)))
                .map(|index| index + 1)
                .unwrap_or(0);
            let style = (face.style != fontdb::Style::Normal) == query.is_italic;
            let weight = -i64::from(u32::from(face.weight.0).abs_diff(query.font_weight));
            let score = (covered, exact, regional, style, weight);
            if best.as_ref().is_none_or(|(_, previous)| score > *previous) {
                best = Some((face.id, score));
            }
        }
        let missing = if best.as_ref().is_none_or(|(_, score)| score.0 < fallback_chars.len()) {
            if let Some(font) = fallback {
                selected.insert(name.clone(), font);
            }
            chars.difference(&fallback_chars).copied().collect::<Vec<_>>()
        } else if let Some((id, _)) = best {
            database
                .with_face_data(id, |data, index| {
                    let font = FontRef::from_index(data, index).unwrap();
                    let cmap = font.charmap();
                    let missing: Vec<_> = chars.iter().filter(|&&c| cmap.map(c).is_none()).copied().collect();
                    selected.insert(name.clone(), (Arc::new(data.to_vec()) as FontData, index));
                    missing
                })
                .unwrap_or_else(|| chars.iter().copied().collect())
        } else {
            chars.iter().copied().collect()
        };
        if !missing.is_empty() {
            let characters = missing
                .into_iter()
                .map(|c| {
                    format!(
                        "{} (U+{:04X})",
                        if c.is_control() { c.escape_default().to_string() } else { c.to_string() },
                        u32::from(c)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            notes.push(format!("{name} is not embedded; its substitute lacks glyphs for {characters}"));
        }
    }
    let mut result = settings.clone();
    let default = settings.font_resolver.clone();
    result.font_resolver = Arc::new(move |query| {
        if let FontQuery::Fallback(query) = query
            && let Some(font) = selected.get(&name(query))
        {
            return Some(font.clone());
        }
        default(query)
    });
    (result, notes)
}

fn coverage(data: &[u8], index: u32, wanted: &BTreeSet<char>) -> BTreeSet<char> {
    if let Ok(font) = FontRef::from_index(data, index) {
        return wanted.iter().filter(|&&c| font.charmap().map(c).is_some()).copied().collect();
    }
    // Hayro's bundled standard fonts are standalone CFF programs. Include
    // their actual glyph-name coverage even when no system fonts are installed.
    use skrifa::raw::ps::cff::{CffFontRef, v1::Cff};
    use skrifa::raw::{FontData, FontRead};
    let mut present = BTreeSet::new();
    if let (Ok(font), Ok(cff)) = (CffFontRef::new(data, 0, None), Cff::read(FontData::new(data)))
        && let Some(charset) = font.charset()
    {
        for (_, sid) in charset.iter() {
            if let Some(bytes) = cff.string(sid)
                && let Ok(name) = std::str::from_utf8(bytes)
                && let Some(text) = pdf_oxide::fonts::font_dict::glyph_name_to_unicode_string(name)
            {
                present.extend(text.chars().filter(|c| wanted.contains(c)));
            }
        }
    }
    present
}

fn name(query: &FallbackFontQuery) -> String {
    query
        .post_script_name
        .as_ref()
        .or(query.font_name.as_ref())
        .or(query.font_family.as_ref())
        .cloned()
        .unwrap_or_else(|| "unnamed PDF font".into())
}

fn normalize(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn regional_names(query: &FallbackFontQuery) -> &[&str] {
    match query.character_collection.as_ref().map(|collection| &collection.family) {
        Some(CidFamily::AdobeGB1) => &[
            "notosanscjksc",
            "notoserifcjksc",
            "sourcesanscn",
            "simsun",
            "microsoftyahei",
            "simhei",
            "pingfangsc",
            "wenquanyi",
        ],
        Some(CidFamily::AdobeCNS1) => {
            &["notosanscjktc", "notoserifcjktc", "sourcesanshc", "mingliu", "microsoftjhenghei", "pingfangtc"]
        }
        Some(CidFamily::AdobeJapan1) => {
            &["notosanscjkjp", "notoserifcjkjp", "yugothic", "yumincho", "msgothic", "msmincho", "hiragino"]
        }
        Some(CidFamily::AdobeKorea1) => {
            &["notosanscjkkr", "notoserifcjkkr", "malgungothic", "batang", "gulim", "applegothic"]
        }
        _ => &[],
    }
}

#[derive(Default)]
struct Used {
    chars: HashMap<String, BTreeSet<char>>,
}
impl<'a> Device<'a> for Used {
    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, _: DrawProps<'a>, mode: &DrawMode) {
        if matches!(mode, DrawMode::Invisible) {
            return;
        }
        for glyph in run.glyphs() {
            if let Glyph::Outline(glyph) = &**glyph
                && let Some(data) = glyph.font_data()
            {
                let name = data.postscript_name.unwrap_or_else(|| "unnamed PDF font".into());
                let chars = self.chars.entry(name).or_default();
                match glyph.as_unicode() {
                    Some(BfString::Char(c)) if !c.is_whitespace() => {
                        chars.insert(c);
                    }
                    Some(BfString::String(text)) => chars.extend(text.chars().filter(|c| !c.is_whitespace())),
                    _ => {}
                }
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
