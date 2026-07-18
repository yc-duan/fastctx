//! Image magic-byte detection and MCP image-content construction for read.

use crate::model::{ToolContent, ToolResponse};
use crate::paths::io_error_message;
use base64::Engine;
use std::fs::File;
use std::io::Read;
use std::path::Path;

const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn detect_image_mime(path: &Path, bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(b"BM") {
        return Some("image/bmp");
    }

    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        Some("bmp") => Some("image/bmp"),
        _ => None,
    }
}

pub(super) fn read_image(path: &Path) -> ToolResponse {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) => return ToolResponse::error(io_error_message(path, &error)),
    };
    let reported_size = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(error) => return ToolResponse::error(io_error_message(path, &error)),
    };
    if reported_size > MAX_IMAGE_BYTES as u64 {
        return oversized_image(reported_size);
    }
    let mut bytes = Vec::with_capacity(reported_size as usize);
    if let Err(error) = file
        .by_ref()
        .take(MAX_IMAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
    {
        return ToolResponse::error(io_error_message(path, &error));
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return oversized_image(bytes.len() as u64);
    }
    let Some(mime_type) = detect_image_mime(path, &bytes) else {
        return ToolResponse::error(format!(
            "Cannot read image file: {}. Retry after confirming it is a PNG, JPG, GIF, WebP, or BMP file.",
            crate::paths::display_path(path)
        ));
    };
    ToolResponse {
        content: vec![ToolContent::Image {
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
            mime_type: mime_type.to_string(),
            detail: None,
        }],
        is_error: false,
    }
}

fn oversized_image(size: u64) -> ToolResponse {
    let mib = ((size as f64 / (1024.0 * 1024.0)) * 10.0).ceil() / 10.0;
    ToolResponse::error(format!(
        "Image file too large: {mib:.1} MiB (limit: 8 MiB). Resize or convert it externally."
    ))
}

#[cfg(test)]
mod tests {
    use super::read_image;
    use crate::ToolContent;

    #[test]
    fn changed_image_format_fails_with_a_retry_path() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("changed.bin");
        std::fs::write(&path, b"not an image anymore").unwrap();
        let response = read_image(&path);
        assert!(response.is_error);
        assert_eq!(
            response.content,
            vec![ToolContent::Text(format!(
                "Cannot read image file: {}. Retry after confirming it is a PNG, JPG, GIF, WebP, or BMP file.",
                crate::paths::display_path(&path)
            ))]
        );
    }
}
