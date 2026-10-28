//! Decode in a single isolated process so malformed images cannot exhaust the node.

use fastctx_types::access::Written;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine as _;
use fastctx_types::access::{Acting, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::thumbnail::{Thumbnail, ThumbnailRequest};

static DECODING: std::sync::LazyLock<crate::ranked::Decoding> = std::sync::LazyLock::new(Default::default);

/// How long one picture may take to decode.
const DECODER: std::time::Duration = std::time::Duration::from_secs(60);

struct Cancel(Arc<AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

struct Worker(Arc<fastctx_platform::child::Child>);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

pub(crate) async fn call(
    node: &Arc<crate::Node>,
    machine: Option<Written>,
    request: ThumbnailRequest,
) -> Result<Thumbnail, Failure> {
    if let Some(machine) = machine.filter(|machine| !node.is_here(machine)) {
        return match node
            .call_with_stream(
                &Acting::Here,
                &machine,
                Purpose::Hands,
                RemoteCall::Thumbnail { request },
                fastctx_net::cancellable::feed,
            )
            .await
            .map_err(|error| Failure::new(error.to_string()))??
        {
            RemoteReply::Thumbnail(thumbnail) => Ok(thumbnail),
            _ => Err(Failure::new("the machine answered something other than a thumbnail")),
        };
    }
    here(node, request, None).await
}

pub(crate) async fn attachment(
    node: &Arc<crate::Node>,
    attachment: fastctx_types::attachments::AttachmentSource,
    max: u32,
) -> Result<Thumbnail, Failure> {
    let machine: Written = attachment.thumbnail_owner().machine.clone().into();
    if !node.is_here(&machine) {
        return match node
            .call_with_stream(
                &Acting::Here,
                &machine,
                Purpose::Agents,
                RemoteCall::AttachmentThumbnail { attachment, max },
                fastctx_net::cancellable::feed,
            )
            .await
            .map_err(|error| Failure::new(error.to_string()))??
        {
            RemoteReply::Thumbnail(thumbnail) => Ok(thumbnail),
            _ => Err(Failure::new("the machine answered something other than an attachment thumbnail")),
        };
    }
    if let fastctx_types::attachments::AttachmentSource::Upload(receipt) = &attachment
        && !node.is_me(&receipt.owner.machine)
    {
        return crate::attachments::preview(node, receipt);
    }
    here(node, ThumbnailRequest { path: String::new(), max }, Some(attachment)).await
}

pub(crate) async fn history(
    node: &Arc<crate::Node>,
    attachment: fastctx_types::attachments::Attachment,
) -> Result<Thumbnail, Failure> {
    use base64::Engine as _;
    for max in [320, 224, 160, 112, 80] {
        let thumbnail =
            here(node, ThumbnailRequest { path: String::new(), max }, Some(attachment.clone().into())).await?;
        if base64::engine::general_purpose::STANDARD
            .decode(&thumbnail.base64)
            .map_err(|e| Failure::new(e.to_string()))?
            .len()
            <= 64 * 1024
        {
            return Ok(thumbnail);
        }
    }
    Err(Failure::new("the history image could not fit in 64 KiB"))
}

async fn here(
    node: &Arc<crate::Node>,
    mut request: ThumbnailRequest,
    attachment: Option<fastctx_types::attachments::AttachmentSource>,
) -> Result<Thumbnail, Failure> {
    let _guard = DECODING.lock().await;
    let program = node.fastctx.clone();
    let node = node.clone();
    let cancelled = Cancel(Arc::new(AtomicBool::new(false)));
    let cancel = cancelled.0.clone();
    let max = request.max;
    let uploaded = match attachment {
        Some(attachment) => Some(crate::attachment_files::resolve(&node, attachment, cancel.clone()).await?),
        None => None,
    };
    if let Some(uploaded) = &uploaded {
        request.path = crate::world::native(&uploaded.path);
    }
    let expected = uploaded.as_ref().map(|uploaded| uploaded.snapshot.clone());
    let (path, before, budget) = tokio::task::spawn_blocking(move || {
        let path = fastctx_platform::local_path(&request.path).map_err(|error| error.to_string())?;
        if !path.is_absolute() {
            return Err("the image path must be absolute".to_string());
        }
        let mut file = fastctx_platform::file_read::open(&path).map_err(|error| error.to_string())?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("the image must be an ordinary file".to_string());
        }
        let before = fastctx_platform::file_read::snapshot(&file, &metadata).map_err(|error| error.to_string())?;
        if expected.as_ref().is_some_and(|uploaded| *uploaded != before) {
            return Err("the uploaded original changed before decoding".to_string());
        }
        let (width, height) = dimensions(&mut file, &cancel)?;
        let budget = allowance(width, height, request.max, before.size)?;
        // OS, resident host, node and decoder each receive one quarter of RAM.
        // Requests decode one at a time, including callers on other machines.
        let available = fastctx_platform::memory::physical().map_err(|error| error.to_string())? / 4;
        if budget > available {
            return Err(format!(
                "image decoding requires a {budget}-byte allowance; this machine can give {available}"
            ));
        }
        Ok::<_, String>((path, before, budget))
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?
    .map_err(Failure::new)?;
    let content = if uploaded.is_some() {
        let source = path.clone();
        Some(
            tokio::task::spawn_blocking(move || fastctx_hands::files::content_fingerprint(&source))
                .await
                .map_err(|error| Failure::new(error.to_string()))?
                .map_err(|error| Failure::new(error.to_string()))?,
        )
    } else {
        None
    };
    let args =
        ["thumbnail-worker".into(), path.as_os_str().to_owned(), max.to_string().into(), budget.to_string().into()];
    let borrowed: Vec<_> = args.iter().map(|arg| arg.as_os_str()).collect();
    let cwd = path.parent().ok_or_else(|| Failure::new("the image has no parent directory"))?;
    let mut child =
        fastctx_platform::child::Child::spawn(&program, &borrowed, cwd, &std::env::vars_os().collect::<Vec<_>>())
            .map_err(|error| Failure::new(error.to_string()))?;
    drop(child.stdin.take());
    let mut stdout = child.stdout.take().expect("worker stdout");
    let mut stderr = child.stderr.take().expect("worker stderr");
    let child = Arc::new(child);
    // This guard ends the owned job/process group when an IPC or network
    // caller leaves, even while blocking readers still await the worker.
    let _worker = Worker(child.clone());
    let reading = tokio::task::spawn_blocking(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let errors = tokio::task::spawn_blocking(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });
    let waiting = tokio::task::spawn_blocking(move || child.wait());
    // Past this, the worker guard ends the decoder as this returns.
    let joined = fastctx_platform::doing::outside("the image decoder", DECODER, async {
        tokio::try_join!(reading, errors, waiting)
    })
    .await
    .map_err(|late| Failure::new(late.to_string()))?;
    let (output, errors, status) = joined.map_err(|error| Failure::new(error.to_string()))?;
    let status = status.map_err(|error| Failure::new(error.to_string()))?;
    let errors = errors.map_err(|error| Failure::new(error.to_string()))?;
    let output = output.map_err(|error| Failure::new(error.to_string()))?;
    if !status.success() {
        return Err(Failure::new(format!("image decoder ended with {status}: {}", String::from_utf8_lossy(&errors))));
    }
    let current = fastctx_platform::file_read::open(&path).map_err(|error| Failure::new(error.to_string()))?;
    let after = fastctx_platform::file_read::snapshot(
        &current,
        &current.metadata().map_err(|error| Failure::new(error.to_string()))?,
    )
    .map_err(|error| Failure::new(error.to_string()))?;
    if before != after {
        return Err(Failure::new("the image changed while decoding"));
    }
    let mut thumbnail: Thumbnail =
        serde_json::from_slice(&output).map_err(|error| Failure::new(format!("invalid thumbnail result: {error}")))?;
    thumbnail.content = content;
    Ok(thumbnail)
}

fn dimensions(file: &mut std::fs::File, cancelled: &AtomicBool) -> Result<(u32, u32), String> {
    let mut header = [0; 32];
    let read = file.read(&mut header).map_err(|error| error.to_string())?;
    if let Some((_, width, height)) = fastctx_hands::image::sniff(&header[..read]) {
        return Ok((width, height));
    }
    if header[..read].starts_with(b"\xff\xd8") {
        file.seek(SeekFrom::Start(2)).map_err(|error| error.to_string())?;
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err("thumbnail was cancelled".into());
            }
            let mut marker = [0; 2];
            file.read_exact(&mut marker).map_err(|error| error.to_string())?;
            if marker[0] != 0xff {
                return Err("invalid JPEG marker".to_string());
            }
            while marker[1] == 0xff {
                file.read_exact(&mut marker[1..]).map_err(|error| error.to_string())?;
            }
            if matches!(marker[1], 0xd9 | 0xda) {
                return Err("JPEG has no frame dimensions".to_string());
            }
            if marker[1] == 0x01 || (0xd0..=0xd7).contains(&marker[1]) {
                continue;
            }
            let mut length = [0; 2];
            file.read_exact(&mut length).map_err(|error| error.to_string())?;
            let length = u16::from_be_bytes(length);
            if length < 2 {
                return Err("invalid JPEG segment length".to_string());
            }
            if (0xc0..=0xcf).contains(&marker[1]) && !matches!(marker[1], 0xc4 | 0xc8 | 0xcc) {
                let mut frame = [0; 5];
                file.read_exact(&mut frame).map_err(|error| error.to_string())?;
                return Ok((
                    u16::from_be_bytes([frame[3], frame[4]]) as u32,
                    u16::from_be_bytes([frame[1], frame[2]]) as u32,
                ));
            }
            file.seek(SeekFrom::Current(i64::from(length - 2))).map_err(|error| error.to_string())?;
        }
    }
    Err("not a supported PNG, JPEG, GIF or WebP image".to_string())
}

fn allowance(width: u32, height: u32, max: u32, encoded: u64) -> Result<u64, String> {
    if width == 0 || height == 0 || max == 0 {
        return Err("image dimensions and max must be positive".to_string());
    }
    let pixels = u64::from(width) * u64::from(height);
    let (small_width, small_height) = size(width, height, max);
    // One RGBA16 source (8 bytes/pixel) and one RGBA32f filtering buffer
    // (16 bytes/pixel), plus 16 bytes per output pixel for encoding. Two encoded copies and one
    // 64 KiB windows account for codec tables and I/O bookkeeping. A tiny
    // image still needs those fixed allocations, independent of its pixels.
    // This is a hard OS allowance, not a claim that every decoder fits in it.
    pixels
        .checked_mul(24)
        .and_then(|n| n.checked_add(u64::from(small_width) * u64::from(small_height) * 16))
        .and_then(|n| encoded.checked_mul(2).and_then(|encoded| n.checked_add(encoded)))
        .and_then(|n| n.checked_add(2 * 64 * 1024))
        .ok_or_else(|| "image memory allowance overflow".to_string())
}

fn size(width: u32, height: u32, max: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= max {
        return (width, height);
    }
    (
        (u64::from(width) * u64::from(max) / u64::from(longest)).max(1) as u32,
        (u64::from(height) * u64::from(max) / u64::from(longest)).max(1) as u32,
    )
}

/// Internal worker entry. It starts no runtime or World and writes only its result.
pub fn worker(path: &Path, max: u32, budget: u64) -> Result<(), String> {
    let _limit = fastctx_platform::memory::limit(budget).map_err(|error| error.to_string())?;
    let file = fastctx_platform::file_read::open(path).map_err(|error| error.to_string())?;
    let mut reader = image::ImageReader::new(std::io::BufReader::new(file))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    if !matches!(
        reader.format(),
        Some(image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::Gif | image::ImageFormat::WebP)
    ) {
        return Err("unsupported image format".to_string());
    }
    let mut limits = image::Limits::no_limits();
    limits.max_alloc = Some(budget);
    reader.limits(limits);
    use image::ImageDecoder as _;
    let mut decoder = reader.into_decoder().map_err(|error| error.to_string())?;
    let orientation = decoder.orientation().map_err(|error| error.to_string())?;
    let mut image = image::DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    image.apply_orientation(orientation);
    let (width, height) = size(image.width(), image.height(), max);
    let small = image.resize_exact(width, height, image::imageops::FilterType::Triangle).to_rgba8();
    drop(image);
    let mut bytes = Vec::new();
    let mime = if small.pixels().any(|pixel| pixel[3] != 255) {
        use image::ImageEncoder as _;
        // The fast encoder keeps alpha with a few kilobytes of state, well
        // inside the allowance's fixed share, where a full DEFLATE encoder's
        // dictionary and hash tables would not be; the filters let it fold
        // flat and smooth areas to almost nothing.
        image::codecs::png::PngEncoder::new_with_quality(
            &mut bytes,
            image::codecs::png::CompressionType::Fast,
            image::codecs::png::FilterType::Adaptive,
        )
        .write_image(small.as_raw(), width, height, image::ExtendedColorType::Rgba8)
        .map_err(|error| error.to_string())?;
        "image/png"
    } else {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 80)
            .encode_image(&small)
            .map_err(|error| error.to_string())?;
        "image/jpeg"
    };
    let thumbnail = Thumbnail {
        content: None,
        width,
        height,
        mime: mime.into(),
        base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    };
    serde_json::to_writer(std::io::stdout().lock(), &thumbnail).map_err(|error| error.to_string())?;
    std::io::stdout().flush().map_err(|error| error.to_string())
}
