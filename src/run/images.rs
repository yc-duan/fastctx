//! Selected image decoding and the host's JSON string-size boundary.

use super::ByteRange;
use super::protocol::{Osc, Scan, kitty_header, osc};
use super::window::Protocol;
use base64::Engine;
use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

// V8's ~536,870,888-character string ceiling must hold one JSON response. 400,000,000
// image bytes encode to 533,333,336 base64 characters; even 150,000 characters escaped
// as six-character JSON sequences leave room for block metadata below that ceiling.
const IMAGE_BYTES: u64 = 400_000_000;

pub(super) struct Image {
    file: tempfile::NamedTempFile,
    pub width: u32,
    pub height: u32,
    original: (u32, u32),
    format: image::ImageFormat,
    pub protocol: Protocol,
}

impl Image {
    pub fn note(&self) -> String {
        let resized = if self.original == (self.width, self.height) {
            String::new()
        } else {
            format!(" (resized from {}x{})", self.original.0, self.original.1)
        };
        format!(
            "\n=== {} image {}x{}{resized} ===\n",
            self.protocol.name(),
            self.width,
            self.height
        )
    }

    pub fn block(&self) -> io::Result<super::ContentBlock> {
        let mut data = String::new();
        let mut input = File::open(self.file.path())?;
        // 3-byte alignment permits independent base64 chunks without interior padding.
        let mut buffer = [0_u8; 3 * 8192];
        loop {
            let mut filled = 0;
            while filled < buffer.len() {
                let read = input.read(&mut buffer[filled..])?;
                if read == 0 {
                    break;
                }
                filled += read;
            }
            if filled == 0 {
                break;
            }
            base64::engine::general_purpose::STANDARD.encode_string(&buffer[..filled], &mut data);
        }
        Ok(super::ContentBlock::Image {
            data,
            mime_type: self.format.to_mime_type().into(),
            width: self.width,
            height: self.height,
        })
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.file.as_file().metadata()?.len())
    }
}

struct Decode {
    file: BufWriter<tempfile::NamedTempFile>,
    quartet: [u8; 4],
    used: usize,
    ended: bool,
    error: Option<io::Error>,
}

impl Decode {
    fn new() -> io::Result<Self> {
        Ok(Self {
            file: BufWriter::new(tempfile::NamedTempFile::new()?),
            quartet: [0; 4],
            used: 0,
            ended: false,
            error: None,
        })
    }
    fn push(&mut self, byte: u8) {
        if self.error.is_some() {
            return;
        }
        if self.ended {
            self.error = Some(io::Error::other("base64 data after padding"));
            return;
        }
        self.quartet[self.used] = byte;
        self.used += 1;
        if self.used == 4 {
            let mut decoded = [0_u8; 3];
            match base64::engine::general_purpose::STANDARD.decode_slice(self.quartet, &mut decoded)
            {
                Ok(length) => {
                    if let Err(error) = self.file.write_all(&decoded[..length]) {
                        self.error = Some(error);
                    }
                    self.ended = length < 3;
                }
                Err(error) => {
                    self.error = Some(io::Error::other(format!("invalid base64: {error}")))
                }
            }
            self.used = 0;
        }
    }
    fn finish(mut self) -> io::Result<tempfile::NamedTempFile> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if self.used != 0 {
            return Err(io::Error::other("truncated base64 quartet"));
        }
        self.file.flush()?;
        self.file.into_inner().map_err(|error| error.into_error())
    }
}

pub(super) fn load(path: &Path, range: &ByteRange, protocol: Protocol) -> io::Result<Image> {
    let mut log = File::open(path)?;
    log.seek(SeekFrom::Start(range.start))?;
    let mut scan = Scan::new(log.take(range.end - range.start));
    let mut decoded = Decode::new()?;
    let mut compressed = false;
    let mut first = true;
    while let Some(byte) = scan.byte()? {
        let byte = if byte == 0xc2 {
            scan.byte()?.unwrap_or(0)
        } else {
            byte
        };
        let control = match byte {
            0x1b => scan.byte()?,
            0x9d => Some(b']'),
            0x9f => Some(b'_'),
            _ => None,
        };
        match (protocol, control) {
            (Protocol::Iterm, Some(b']')) => match osc(&mut scan)? {
                Osc::File => {
                    let (_, payload) = scan.header(b':', b"")?;
                    if !payload || !scan.string(true, |byte| decoded.push(byte))? {
                        return Err(io::Error::other("truncated OSC 1337 image"));
                    }
                }
                Osc::Part => {
                    if !scan.string(true, |byte| decoded.push(byte))? {
                        return Err(io::Error::other("truncated OSC 1337 image part"));
                    }
                }
                Osc::Multipart | Osc::End | Osc::Other => {
                    scan.string(true, |_| {})?;
                }
            },
            (Protocol::Kitty, Some(b'_')) if scan.byte()? == Some(b'G') => {
                let (header, payload) = kitty_header(&mut scan)?;
                if first {
                    if !header.png || !header.direct {
                        return Err(io::Error::other(
                            "kitty image requires inline PNG data (f=100,t=d)",
                        ));
                    }
                    compressed = header.compressed;
                    first = false;
                } else if !header.continuation {
                    return Err(io::Error::other("damaged kitty image continuation"));
                }
                if !payload || !scan.string(false, |byte| decoded.push(byte))? {
                    return Err(io::Error::other("truncated kitty image"));
                }
            }
            _ => (),
        }
    }
    let mut file = decoded.finish()?;
    if compressed {
        let mut inflated = tempfile::NamedTempFile::new()?;
        let mut input = flate2::read::ZlibDecoder::new(File::open(file.path())?);
        io::copy(&mut input, &mut inflated)?;
        inflated.flush()?;
        file = inflated;
    }
    let mut reader = image::ImageReader::open(file.path())?.with_guessed_format()?;
    reader.no_limits();
    let format = reader
        .format()
        .ok_or_else(|| io::Error::other("unrecognized image data"))?;
    if protocol == Protocol::Kitty && format != image::ImageFormat::Png {
        return Err(io::Error::other("kitty f=100 payload is not PNG"));
    }
    let (width, height) = reader.into_dimensions().map_err(io::Error::other)?;
    if format == image::ImageFormat::Png {
        super::png::validate(file.path())?;
    }
    Ok(Image {
        file,
        width,
        height,
        original: (width, height),
        format,
        protocol,
    })
}

pub(super) fn fit(images: &mut [&mut Image]) -> io::Result<()> {
    loop {
        let sizes: Vec<_> = images
            .iter()
            .map(|image| image.size())
            .collect::<io::Result<_>>()?;
        let total: u64 = sizes.iter().sum();
        if total <= IMAGE_BYTES {
            return Ok(());
        }
        let (index, largest) = sizes
            .iter()
            .enumerate()
            .max_by_key(|(_, size)| *size)
            .expect("positive total has an image");
        let image = &mut images[index];
        // Share the available bytes proportionally. Pixel count scales with the square
        // of a dimension; actual encoded bytes are measured again after each reduction.
        let target = (*largest as f64 * IMAGE_BYTES as f64 / total as f64).floor();
        let factor = (target / *largest as f64).sqrt();
        let width = ((image.width as f64 * factor).floor() as u32).max(1);
        let height = ((image.height as f64 * factor).floor() as u32).max(1);
        if (width, height) == (image.width, image.height) {
            return Err(io::Error::other(
                "image cannot be reduced to fit the host JSON limit",
            ));
        }
        let mut reader = image::ImageReader::open(image.file.path())?.with_guessed_format()?;
        reader.no_limits();
        let pixels = reader.decode().map_err(io::Error::other)?;
        let resized = pixels.resize_exact(width, height, image::imageops::FilterType::Triangle);
        let mut file = tempfile::NamedTempFile::new()?;
        resized
            .write_to(file.as_file_mut(), image::ImageFormat::Png)
            .map_err(io::Error::other)?;
        image.file = file;
        image.width = width;
        image.height = height;
        image.format = image::ImageFormat::Png;
    }
}
