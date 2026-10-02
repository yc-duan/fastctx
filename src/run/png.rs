//! PNG integrity validation without allocating a raster proportional to image dimensions.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

pub(super) fn validate(path: &Path) -> io::Result<()> {
    let mut source = BufReader::new(File::open(path)?);
    let mut signature = [0; 8];
    source.read_exact(&mut signature)?;
    if signature != *b"\x89PNG\r\n\x1a\n" {
        return Err(invalid("invalid PNG signature"));
    }
    let mut header = None;
    let mut data = BufWriter::new(tempfile::NamedTempFile::new()?);
    let mut has_data = false;
    loop {
        let mut framing = [0; 8];
        source.read_exact(&mut framing)?;
        let length = u32::from_be_bytes(framing[..4].try_into().unwrap());
        let kind = &framing[4..];
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        if header.is_none() && kind != b"IHDR" {
            return Err(invalid("PNG must begin with IHDR"));
        }
        let mut remaining = length as usize;
        let mut buffer = [0_u8; 8192];
        if kind == b"IHDR" {
            if header.is_some() || length != 13 {
                return Err(invalid("invalid PNG IHDR"));
            }
            let mut bytes = [0; 13];
            source.read_exact(&mut bytes)?;
            crc.update(&bytes);
            header = Some(bytes);
            remaining = 0;
        }
        if kind == b"IDAT" {
            has_data = true;
        }
        while remaining > 0 {
            let count = remaining.min(buffer.len());
            source.read_exact(&mut buffer[..count])?;
            crc.update(&buffer[..count]);
            if kind == b"IDAT" {
                data.write_all(&buffer[..count])?;
            }
            remaining -= count;
        }
        let mut checksum = [0; 4];
        source.read_exact(&mut checksum)?;
        if crc.sum() != u32::from_be_bytes(checksum) {
            return Err(invalid("PNG chunk CRC mismatch"));
        }
        if kind == b"IEND" {
            if length != 0 || !has_data {
                return Err(invalid("invalid PNG image end"));
            }
            break;
        }
    }
    data.flush()?;
    let mut pixels = flate2::read::ZlibDecoder::new(File::open(data.get_ref().path())?);
    let header = header.expect("IHDR checked before IEND");
    let width = u64::from(u32::from_be_bytes(header[..4].try_into().unwrap()));
    let height = u64::from(u32::from_be_bytes(header[4..8].try_into().unwrap()));
    let channels = match header[9] {
        0 | 3 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => return Err(invalid("invalid PNG color type")),
    };
    let bits = u64::from(header[8]) * channels;
    // Adam7's seven passes are defined by PNG's interlace method 1. Each transmitted
    // row has one filter byte followed by ceil(pass_width * bits_per_pixel / 8) bytes.
    let passes: &[(u64, u64, u64, u64)] = match header[12] {
        0 => &[(0, 0, 1, 1)],
        1 => &[
            (0, 0, 8, 8),
            (4, 0, 8, 8),
            (0, 4, 4, 8),
            (2, 0, 4, 4),
            (0, 2, 2, 4),
            (1, 0, 2, 2),
            (0, 1, 1, 2),
        ],
        _ => return Err(invalid("invalid PNG interlace method")),
    };
    for &(x, y, dx, dy) in passes {
        let columns = width.saturating_sub(x).div_ceil(dx);
        let rows = height.saturating_sub(y).div_ceil(dy);
        if columns == 0 {
            continue;
        }
        let row_bytes = (columns * bits).div_ceil(8);
        for _ in 0..rows {
            let mut filter = [0];
            pixels.read_exact(&mut filter)?;
            if filter[0] > 4 {
                return Err(invalid("invalid PNG row filter"));
            }
            if io::copy(&mut (&mut pixels).take(row_bytes), &mut io::sink())? != row_bytes {
                return Err(invalid("truncated PNG row"));
            }
        }
    }
    if pixels.read(&mut [0])? != 0 {
        return Err(invalid("excess PNG pixel data"));
    }
    Ok(())
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
