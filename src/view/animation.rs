//! Preserve animated PNG and WebP frames when the image-byte limit requires resizing.

use image::{
    AnimationDecoder, ImageDecoder, ImageFormat,
    codecs::{png::PngDecoder, webp::WebPDecoder},
};
use std::io::{BufRead, Cursor, Read, Seek, SeekFrom};

pub(super) fn resize(
    mut source: impl BufRead + Seek,
    format: ImageFormat,
    width: u32,
    height: u32,
) -> Result<Option<Vec<u8>>, String> {
    let Some(plays) = plays(&mut source, format).map_err(|error| error.to_string())? else {
        return Ok(None);
    };
    source.rewind().map_err(|error| error.to_string())?;
    let output = if format == ImageFormat::Png {
        let decoder = PngDecoder::with_limits(source, image::Limits::no_limits())
            .map_err(|error| error.to_string())?;
        encode(
            decoder
                .apng()
                .map_err(|error| error.to_string())?
                .into_frames(),
            width,
            height,
            plays,
        )?
    } else {
        let mut decoder = WebPDecoder::new(source).map_err(|error| error.to_string())?;
        decoder
            .set_limits(image::Limits::no_limits())
            .map_err(|error| error.to_string())?;
        encode(decoder.into_frames(), width, height, plays)?
    };
    Ok(Some(output))
}

fn plays(source: &mut (impl Read + Seek), format: ImageFormat) -> std::io::Result<Option<u32>> {
    if !matches!(format, ImageFormat::Png | ImageFormat::WebP) {
        return Ok(None);
    }
    source.seek(SeekFrom::Start(if format == ImageFormat::Png {
        8
    } else {
        12
    }))?;
    loop {
        let mut header = [0; 8];
        source.read_exact(&mut header)?;
        if format == ImageFormat::Png {
            let length = u32::from_be_bytes(header[..4].try_into().unwrap());
            match &header[4..] {
                b"acTL" => {
                    let mut control = [0; 8];
                    source.read_exact(&mut control)?;
                    return Ok(Some(u32::from_be_bytes(control[4..].try_into().unwrap())));
                }
                b"IDAT" | b"IEND" => return Ok(None),
                _ => {
                    source.seek(SeekFrom::Current(i64::from(length) + 4))?;
                }
            }
        } else {
            let length = u32::from_le_bytes(header[4..].try_into().unwrap());
            match &header[..4] {
                b"ANIM" => {
                    let mut control = [0; 6];
                    source.read_exact(&mut control)?;
                    return Ok(Some(u32::from(u16::from_le_bytes([
                        control[4], control[5],
                    ]))));
                }
                b"VP8 " | b"VP8L" => return Ok(None),
                _ => {
                    source.seek(SeekFrom::Current(i64::from(length) + i64::from(length % 2)))?;
                }
            }
        }
    }
}

fn chunk(output: &mut Vec<u8>, kind: &[u8; 4], bytes: &[u8]) {
    output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    output.extend_from_slice(kind);
    output.extend_from_slice(bytes);
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(bytes);
    output.extend_from_slice(&crc.sum().to_be_bytes());
}

fn encode(
    frames: image::Frames<'_>,
    width: u32,
    height: u32,
    plays: u32,
) -> Result<Vec<u8>, String> {
    let mut output = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::from(width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut output, b"IHDR", &header);
    let control_offset = output.len();
    chunk(&mut output, b"acTL", &[0; 8]);
    let mut sequence = 0_u32;
    let mut count = 0_u32;
    for frame in frames {
        let frame = frame.map_err(|error| error.to_string())?;
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let mut numerator = u64::from(numerator);
        let mut denominator = u64::from(denominator) * 1000;
        let (mut a, mut b) = (numerator, denominator);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        numerator /= a;
        denominator /= a;
        let denominator = u16::try_from(denominator)
            .map_err(|_| "Animation delay cannot be represented in PNG.".to_string())?;
        let resized = image::imageops::resize(
            frame.buffer(),
            width,
            height,
            image::imageops::FilterType::Triangle,
        );
        let mut encoded = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(resized)
            .write_to(&mut encoded, ImageFormat::Png)
            .map_err(|error| error.to_string())?;
        let png = encoded.into_inner();
        // PNG delays have 16-bit numerators. Repeating an identical full-canvas
        // frame preserves longer WebP delays exactly without changing visible motion.
        loop {
            let delay = numerator.min(u64::from(u16::MAX)) as u16;
            let mut control = Vec::from(sequence.to_be_bytes());
            sequence = sequence
                .checked_add(1)
                .ok_or("PNG animation sequence overflow.")?;
            control.extend_from_slice(&width.to_be_bytes());
            control.extend_from_slice(&height.to_be_bytes());
            control.extend_from_slice(&[0; 8]);
            control.extend_from_slice(&delay.to_be_bytes());
            control.extend_from_slice(&denominator.to_be_bytes());
            control.extend_from_slice(&[0, 0]);
            chunk(&mut output, b"fcTL", &control);
            let mut position = 8;
            while position < png.len() {
                let length =
                    u32::from_be_bytes(png[position..position + 4].try_into().unwrap()) as usize;
                if &png[position + 4..position + 8] == b"IDAT" {
                    let data = &png[position + 8..position + 8 + length];
                    if count == 0 {
                        chunk(&mut output, b"IDAT", data);
                    } else {
                        let mut data_with_sequence = Vec::from(sequence.to_be_bytes());
                        sequence = sequence
                            .checked_add(1)
                            .ok_or("PNG animation sequence overflow.")?;
                        data_with_sequence.extend_from_slice(data);
                        chunk(&mut output, b"fdAT", &data_with_sequence);
                    }
                }
                position += length + 12;
            }
            count = count
                .checked_add(1)
                .ok_or("PNG animation frame count overflow.")?;
            numerator -= u64::from(delay);
            if numerator == 0 {
                break;
            }
        }
    }
    if count == 0 {
        return Err("Animation has no frames.".to_string());
    }
    chunk(&mut output, b"IEND", &[]);
    let mut control = Vec::from(count.to_be_bytes());
    control.extend_from_slice(&plays.to_be_bytes());
    let mut control_chunk = Vec::new();
    chunk(&mut control_chunk, b"acTL", &control);
    output[control_offset..control_offset + control_chunk.len()].copy_from_slice(&control_chunk);
    Ok(output)
}
