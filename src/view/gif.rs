//! Resizing GIF frames without losing their timing or repetition metadata.

use image::{
    AnimationDecoder, Frame,
    codecs::gif::{GifDecoder, GifEncoder, Repeat},
};
use std::io::{BufRead, Read, Seek, SeekFrom};

pub(super) fn resize(
    mut source: impl BufRead + Seek,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let repeat = repeat(&mut source).map_err(|error| format!("Cannot read GIF: {error}"))?;
    source.rewind().map_err(|error| error.to_string())?;
    let decoder = GifDecoder::new(source).map_err(|error| error.to_string())?;
    let mut output = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut output);
        if let Some(repeat) = repeat {
            encoder
                .set_repeat(repeat)
                .map_err(|error| error.to_string())?;
        }
        for frame in decoder.into_frames() {
            let frame = frame.map_err(|error| error.to_string())?;
            let delay = frame.delay();
            let resized = image::imageops::resize(
                frame.buffer(),
                width,
                height,
                image::imageops::FilterType::Triangle,
            );
            encoder
                .encode_frame(Frame::from_parts(resized, 0, 0, delay))
                .map_err(|error| error.to_string())?;
        }
    }
    if output.is_empty() {
        return Err("GIF has no frames.".to_string());
    }
    Ok(output)
}

fn byte(source: &mut impl Read) -> std::io::Result<u8> {
    let mut byte = [0];
    source.read_exact(&mut byte)?;
    Ok(byte[0])
}

fn skip_blocks(source: &mut (impl Read + Seek)) -> std::io::Result<()> {
    loop {
        let count = byte(source)?;
        if count == 0 {
            return Ok(());
        }
        source.seek(SeekFrom::Current(i64::from(count)))?;
    }
}

fn repeat(source: &mut (impl Read + Seek)) -> std::io::Result<Option<Repeat>> {
    let mut header = [0; 13];
    source.read_exact(&mut header)?;
    if header[10] & 0x80 != 0 {
        source.seek(SeekFrom::Current(3 * (1 << ((header[10] & 7) + 1))))?;
    }
    loop {
        match byte(source)? {
            0x3b => return Ok(None),
            0x2c => {
                let mut descriptor = [0; 9];
                source.read_exact(&mut descriptor)?;
                if descriptor[8] & 0x80 != 0 {
                    source.seek(SeekFrom::Current(3 * (1 << ((descriptor[8] & 7) + 1))))?;
                }
                byte(source)?;
                skip_blocks(source)?;
            }
            0x21 => {
                let label = byte(source)?;
                if label == 0xff {
                    let count = byte(source)?;
                    let mut application = vec![0; count as usize];
                    source.read_exact(&mut application)?;
                    let looping = application == b"NETSCAPE2.0" || application == b"ANIMEXTS1.0";
                    loop {
                        let count = byte(source)?;
                        if count == 0 {
                            break;
                        }
                        let mut block = vec![0; count as usize];
                        source.read_exact(&mut block)?;
                        if looping && block.len() == 3 && block[0] == 1 {
                            let count = u16::from_le_bytes([block[1], block[2]]);
                            return Ok(Some(if count == 0 {
                                Repeat::Infinite
                            } else {
                                Repeat::Finite(count)
                            }));
                        }
                    }
                } else {
                    skip_blocks(source)?;
                }
            }
            _ => return Err(std::io::Error::other("invalid GIF block")),
        }
    }
}
