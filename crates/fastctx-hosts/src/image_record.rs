//! Read one host JSONL record at a time, spooling encoded images to disk.
//! No image-sized string or decoded byte buffer is allocated by the node.

use serde_json::Value;
use std::io::{self, BufRead, Read, Seek, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Record {
    pub value: Value,
    blobs: Vec<tempfile::NamedTempFile>,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// Image fields are replaced by private blob markers before JSON parsing.
/// Metadata receives the node's quarter-RAM share, charging four Value
/// slots per encoded byte for parsing, containers and their growth.
pub fn next(reader: &mut impl BufRead, temporary: &Path, cancelled: &AtomicBool) -> io::Result<Option<Record>> {
    let allowance = fastctx_platform::memory::physical()? / 4 / (4 * std::mem::size_of::<Value>() as u64);
    let mut json = Vec::new();
    let mut blobs = Vec::new();
    let mut key = String::new();
    let mut value_of = String::new();
    let mut containers: Vec<(u8, bool)> = Vec::new();
    let mut image_arrays = Vec::new();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "image read was cancelled"));
        }
        let Some(byte) = byte(reader)? else {
            if json.iter().all(u8::is_ascii_whitespace) {
                return Ok(None);
            }
            break;
        };
        if byte == b'\n' {
            if json.iter().all(u8::is_ascii_whitespace) {
                json.clear();
                continue;
            }
            break;
        }
        if byte == b'"' {
            let is_key = containers.last().is_some_and(|(kind, key)| *kind == b'{' && *key);
            let image_string = image_arrays.last() == Some(&containers.len())
                && containers.last().is_some_and(|(kind, _)| *kind == b'[');
            if !is_key && (matches!(value_of.as_str(), "data" | "image_url" | "url") || image_string) {
                let mut blob = tempfile::NamedTempFile::new_in(temporary)?;
                string(reader, |bytes| blob.write_all(bytes), cancelled)?;
                blob.flush()?;
                let marker = serde_json::to_vec(&serde_json::json!({"$world_image_blob":blobs.len()}))
                    .map_err(io::Error::other)?;
                json.extend(marker);
                blobs.push(blob);
                key.clear();
            } else {
                let mut encoded = Vec::from([b'"']);
                // Preserve ordinary strings exactly. serde_json validates
                // their escapes and UTF-8 along with the enclosing record.
                loop {
                    let b = byte_required(reader)?;
                    encoded.push(b);
                    if encoded.len() as u64 + json.len() as u64 > allowance {
                        return Err(invalid("host record metadata exceeds this machine's parsing allowance"));
                    }
                    if b == b'\\' {
                        encoded.push(byte_required(reader)?);
                    } else if b == b'"' {
                        break;
                    }
                }
                if is_key {
                    key = serde_json::from_slice(&encoded).map_err(io::Error::other)?;
                }
                json.extend(encoded);
            }
            value_of.clear();
        } else {
            json.push(byte);
            match byte {
                b'{' => containers.push((b'{', true)),
                b'[' => {
                    containers.push((b'[', false));
                    if value_of == "images" {
                        image_arrays.push(containers.len());
                    }
                }
                b'}' | b']' => {
                    if image_arrays.last() == Some(&containers.len()) {
                        image_arrays.pop();
                    }
                    containers.pop();
                }
                b',' => {
                    if let Some((b'{', key)) = containers.last_mut() {
                        *key = true;
                    }
                }
                b':' => {
                    if let Some((b'{', key)) = containers.last_mut() {
                        *key = false;
                    }
                }
                _ => {}
            }
            if byte == b':' {
                value_of = std::mem::take(&mut key);
            } else if !byte.is_ascii_whitespace() {
                value_of.clear();
                key.clear();
            }
        }
        if json.len() as u64 > allowance {
            return Err(invalid("host record metadata exceeds this machine's parsing allowance"));
        }
    }
    let value = serde_json::from_slice(&json).map_err(io::Error::other)?;
    Ok(Some(Record { value, blobs }))
}

fn byte(reader: &mut impl BufRead) -> io::Result<Option<u8>> {
    let bytes = reader.fill_buf()?;
    let byte = bytes.first().copied();
    if byte.is_some() {
        reader.consume(1);
    }
    Ok(byte)
}
fn byte_required(reader: &mut impl BufRead) -> io::Result<u8> {
    byte(reader)?.ok_or_else(|| invalid("host record ended inside a string"))
}

fn string(
    reader: &mut impl BufRead,
    mut output: impl FnMut(&[u8]) -> io::Result<()>,
    cancelled: &AtomicBool,
) -> io::Result<()> {
    let mut block = Vec::with_capacity(8192);
    loop {
        let b = byte_required(reader)?;
        if b == b'"' {
            output(&block)?;
            return Ok(());
        }
        if b < 0x20 {
            return Err(invalid("unescaped control character in host image"));
        }
        if b == b'\\' {
            let mut escape = Vec::from([b'"', b'\\', byte_required(reader)?]);
            if escape[2] == b'u' {
                for _ in 0..4 {
                    escape.push(byte_required(reader)?);
                }
                let first = u16::from_str_radix(std::str::from_utf8(&escape[3..]).map_err(io::Error::other)?, 16)
                    .map_err(io::Error::other)?;
                if (0xd800..=0xdbff).contains(&first) {
                    for _ in 0..6 {
                        escape.push(byte_required(reader)?);
                    }
                }
            }
            escape.push(b'"');
            let decoded: String = serde_json::from_slice(&escape).map_err(io::Error::other)?;
            block.extend_from_slice(decoded.as_bytes());
        } else {
            block.push(b);
        }
        if block.len() >= 8192 {
            if cancelled.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "image read was cancelled"));
            }
            output(&block)?;
            block.clear();
        }
    }
}

impl Record {
    /// Decode just this image into a caller-owned temporary file. Its host
    /// may have resized it; these are not asserted to be upload bytes.
    pub fn decode(
        &mut self,
        value: &Value,
        data_url: bool,
        destination: &mut impl Write,
        cancelled: &AtomicBool,
    ) -> io::Result<String> {
        if value.get("file_id").is_some() || value.get("fileId").is_some() {
            return Err(invalid(
                "Codex kept a provider file reference; its public app-server cannot resolve the picture's bytes",
            ));
        }
        let index = value
            .get("$world_image_blob")
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid("host image has no encoded bytes"))?;
        let blob = self.blobs.get_mut(index as usize).ok_or_else(|| invalid("host image refers to an unknown blob"))?;
        blob.as_file_mut().rewind()?;
        let mut encoded = std::io::BufReader::new(blob.as_file_mut());
        let mime = if data_url {
            let mut head = Vec::new();
            let longest = ["image/png", "image/jpeg", "image/gif", "image/webp"]
                .iter()
                .map(|mime| "data:".len() + mime.len() + ";base64,".len())
                .max()
                .expect("supported MIME types");
            if encoded.by_ref().take(longest as u64).read_until(b',', &mut head)? == 0 {
                return Err(invalid("host image has no data URL header"));
            }
            let head = std::str::from_utf8(&head).map_err(io::Error::other)?;
            head.strip_prefix("data:")
                .and_then(|head| head.strip_suffix(";base64,"))
                .filter(|mime| mime.starts_with("image/"))
                .ok_or_else(|| invalid("host image is not an embedded base64 image"))?
                .to_string()
        } else {
            String::new()
        };
        let mut decoded = base64::read::DecoderReader::new(encoded, &base64::engine::general_purpose::STANDARD);
        let mut block = [0; 8192];
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "image read was cancelled"));
            }
            let read = decoded.read(&mut block)?;
            if read == 0 {
                break;
            }
            destination.write_all(&block[..read])?;
        }
        destination.flush()?;
        Ok(mime)
    }
}
