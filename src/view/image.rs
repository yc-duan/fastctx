//! Original image transport and the response-wide V8 string limit.

use base64::write::EncoderWriter;
use image::{DynamicImage, ImageFormat, ImageReader};
use std::fs::File;
use std::io::{self, BufReader, Cursor, Read, Seek, SeekFrom, Write};
use std::sync::Arc;

use super::{CHARACTER_BUDGET, IMAGE_CHARACTERS};
use crate::head_note::{HeadMetric, HeadNote};

#[derive(Clone)]
enum Payload {
    File(Arc<File>),
    Encoded(Arc<Vec<u8>>, ImageFormat),
}

#[derive(Clone)]
pub(super) struct Picture {
    subject: String,
    dimensions: (u32, u32),
    current_dimensions: (u32, u32),
    pub(super) bytes: u64,
    payload: Payload,
    resized: bool,
    format: ImageFormat,
}

impl Picture {
    pub(super) fn open(subject: &str, mut file: File) -> Result<Self, String> {
        let bytes = file.metadata().map_err(|error| error.to_string())?.len();
        let mut reader = ImageReader::new(BufReader::new(&mut file))
            .with_guessed_format()
            .map_err(|error| error.to_string())?;
        reader.no_limits();
        let format = reader.format().ok_or("Cannot determine image format.")?;
        let dimensions = reader
            .into_dimensions()
            .map_err(|error| format!("Cannot read image: {error}"))?;
        file.rewind().map_err(|error| error.to_string())?;
        Ok(Self {
            subject: subject.to_string(),
            dimensions,
            current_dimensions: dimensions,
            bytes,
            payload: Payload::File(Arc::new(file)),
            resized: false,
            format,
        })
    }

    #[cfg(feature = "pdf")]
    pub(super) fn rendered(subject: &str, image: DynamicImage) -> Result<Self, String> {
        let dimensions = (image.width(), image.height());
        let bytes = png(&image)?;
        Ok(Self {
            subject: subject.to_string(),
            dimensions,
            current_dimensions: dimensions,
            bytes: bytes.len() as u64,
            payload: Payload::Encoded(Arc::new(bytes), ImageFormat::Png),
            resized: false,
            format: ImageFormat::Png,
        })
    }

    pub(super) fn note(&self) -> String {
        let mut note = HeadNote::new(
            &self.subject,
            HeadMetric::event(format!("image {}x{}", self.dimensions.0, self.dimensions.1)),
        );
        if self.resized {
            note = note.fact(format!(
                "resized to {}x{}",
                self.current_dimensions.0, self.current_dimensions.1
            ));
        }
        format!("{}\n", note.render())
    }

    pub(super) fn cost(&self) -> usize {
        // The image sequence is followed by a newline outside the OSC payload.
        self.note().chars().count() + IMAGE_CHARACTERS + 1
    }

    pub(super) fn resize(&mut self, target_bytes: u64) -> Result<(), String> {
        if self.format == ImageFormat::Gif {
            return self.resize_gif(target_bytes);
        }
        if matches!(self.format, ImageFormat::Png | ImageFormat::WebP)
            && self.resize_animation(target_bytes)?
        {
            return Ok(());
        }
        let mut decoded = match &mut self.payload {
            Payload::File(file) => {
                let mut file = file.as_ref();
                file.rewind().map_err(|error| error.to_string())?;
                let mut reader = ImageReader::new(BufReader::new(file))
                    .with_guessed_format()
                    .map_err(|error| error.to_string())?;
                reader.no_limits();
                reader
                    .decode()
                    .map_err(|error| format!("Cannot resize image: {error}"))?
            }
            Payload::Encoded(bytes, format) => {
                let mut reader = ImageReader::with_format(Cursor::new(bytes.as_ref()), *format);
                reader.no_limits();
                reader
                    .decode()
                    .map_err(|error| format!("Cannot resize image: {error}"))?
            }
        };
        let mut encoded_size = self.bytes;
        loop {
            let factor = (target_bytes as f64 / encoded_size as f64).sqrt();
            let width = ((decoded.width() as f64 * factor).floor() as u32).max(1);
            let height = ((decoded.height() as f64 * factor).floor() as u32).max(1);
            let unchanged = width == decoded.width() && height == decoded.height();
            decoded = decoded.resize_exact(width, height, image::imageops::FilterType::Triangle);
            let bytes = png(&decoded)?;
            encoded_size = bytes.len() as u64;
            if encoded_size <= target_bytes {
                self.current_dimensions = (width, height);
                self.bytes = encoded_size;
                self.payload = Payload::Encoded(Arc::new(bytes), ImageFormat::Png);
                self.resized = true;
                return Ok(());
            }
            if unchanged {
                return Err("Cannot shrink image within the response image byte limit.".to_string());
            }
        }
    }

    fn resize_gif(&mut self, target_bytes: u64) -> Result<(), String> {
        let mut encoded_size = self.bytes;
        let (mut width, mut height) = self.current_dimensions;
        loop {
            let factor = (target_bytes as f64 / encoded_size as f64).sqrt();
            let new_width = (width as f64 * factor).floor().max(1.0) as u32;
            let new_height = (height as f64 * factor).floor().max(1.0) as u32;
            let bytes = match &mut self.payload {
                Payload::File(file) => {
                    let mut file = file.as_ref();
                    file.rewind().map_err(|error| error.to_string())?;
                    super::gif::resize(BufReader::new(file), new_width, new_height)?
                }
                Payload::Encoded(bytes, _) => {
                    super::gif::resize(Cursor::new(bytes.as_ref()), new_width, new_height)?
                }
            };
            encoded_size = bytes.len() as u64;
            if encoded_size <= target_bytes {
                self.current_dimensions = (new_width, new_height);
                self.bytes = encoded_size;
                self.payload = Payload::Encoded(Arc::new(bytes), ImageFormat::Gif);
                self.resized = true;
                return Ok(());
            }
            if new_width == width && new_height == height {
                return Err("Cannot shrink GIF within the response image byte limit.".to_string());
            }
            (width, height) = (new_width, new_height);
        }
    }

    fn resize_animation(&mut self, target_bytes: u64) -> Result<bool, String> {
        let mut encoded_size = self.bytes;
        let (mut width, mut height) = self.current_dimensions;
        loop {
            let factor = (target_bytes as f64 / encoded_size as f64).sqrt();
            let new_width = (width as f64 * factor).floor().max(1.0) as u32;
            let new_height = (height as f64 * factor).floor().max(1.0) as u32;
            let bytes = match &mut self.payload {
                Payload::File(file) => {
                    let mut file = file.as_ref();
                    file.rewind().map_err(|error| error.to_string())?;
                    super::animation::resize(
                        BufReader::new(file),
                        self.format,
                        new_width,
                        new_height,
                    )?
                }
                Payload::Encoded(bytes, format) => super::animation::resize(
                    Cursor::new(bytes.as_ref()),
                    *format,
                    new_width,
                    new_height,
                )?,
            };
            let Some(bytes) = bytes else {
                return Ok(false);
            };
            encoded_size = bytes.len() as u64;
            if encoded_size <= target_bytes {
                self.current_dimensions = (new_width, new_height);
                self.bytes = encoded_size;
                self.payload = Payload::Encoded(Arc::new(bytes), ImageFormat::Png);
                self.format = ImageFormat::Png;
                self.resized = true;
                return Ok(true);
            }
            if new_width == width && new_height == height {
                return Err(
                    "Cannot shrink animation within the response image byte limit.".to_string(),
                );
            }
            (width, height) = (new_width, new_height);
        }
    }

    pub(super) fn write(&mut self, output: &mut impl Write) -> io::Result<()> {
        output.write_all(self.note().as_bytes())?;
        write!(
            output,
            "\x1b]1337;File=inline=1;size={};preserveAspectRatio=1:",
            self.bytes
        )?;
        let mut encoder =
            EncoderWriter::new(&mut *output, &base64::engine::general_purpose::STANDARD);
        match &mut self.payload {
            Payload::File(file) => {
                let mut file = file.as_ref();
                file.seek(SeekFrom::Start(0))?;
                let copied = io::copy(&mut file.take(self.bytes), &mut encoder)?;
                if copied != self.bytes {
                    return Err(io::Error::other("image changed while being read"));
                }
            }
            Payload::Encoded(bytes, _) => encoder.write_all(bytes)?,
        }
        encoder.finish()?;
        drop(encoder);
        output.write_all(b"\x07\n")
    }
}

fn png(image: &DynamicImage) -> Result<Vec<u8>, String> {
    let mut output = Cursor::new(Vec::new());
    image
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok(output.into_inner())
}

pub(super) fn shrink(pictures: &mut [&mut Picture]) -> Result<(), (usize, String)> {
    loop {
        let total: u64 = pictures.iter().map(|picture| picture.bytes).sum();
        let limit = byte_limit(pictures.len());
        if total <= limit {
            return Ok(());
        }
        let index = pictures
            .iter()
            .enumerate()
            .max_by_key(|(_, picture)| picture.bytes)
            .unwrap()
            .0;
        // The equal share prevents several large images from forcing the first
        // one below the minimum size of an encoded image.
        let target = pictures[index]
            .bytes
            .saturating_sub(total - limit)
            .max(limit / pictures.len() as u64);
        pictures[index]
            .resize(target)
            .map_err(|error| (index, error))?;
    }
}

/// V8 permits 2^29-24 string code units. Text can expand to six JSON characters
/// per scalar; each image's JSON envelope is measured with serde_json. Base64
/// requires four characters per three bytes, rounded independently per image.
pub(super) fn byte_limit(images: usize) -> u64 {
    let envelope = serde_json::json!({"type":"image","mimeType":"image/webp","data":""})
        .to_string()
        .len();
    // Text block and response envelopes, including punctuation, are measured too.
    let response = serde_json::json!({"jsonrpc":"2.0","id":u64::MAX,"result":{"content":[{"type":"text","text":""}],"isError":false}}).to_string().len();
    let json_chars =
        (1_usize << 29) - 24 - CHARACTER_BUDGET * 6 - response - images * (envelope + 1 + 4);
    (json_chars / 4 * 3) as u64
}
