//! Thumbnails made here, kept by what they were made from and their size,
//! so the same picture asked again at the same size is not decoded again.
//! A picture is known by its content's fingerprint when it has one, else by
//! its file's identity and change indicators; a file changed within the
//! last two seconds is not kept, as two writes within one tick of the
//! file system's clock could leave its indicators the same.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use fastctx_types::thumbnail::Thumbnail;

/// The most bytes of thumbnails kept.
const BYTES: usize = 32 << 20;
/// How recently a file may have changed and still be kept by its indicators.
const SETTLED: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Cache {
    entries: HashMap<String, (u64, Thumbnail)>,
    bytes: usize,
    tick: u64,
}

static CACHE: std::sync::LazyLock<Mutex<Cache>> = std::sync::LazyLock::new(Default::default);

/// The key of a picture with the content fingerprint `content`.
pub(crate) fn of_content(content: &fastctx_types::session::Fingerprint, max: u32) -> String {
    format!("content {} {} {max}", content.sha256, content.size)
}

/// The key of the file at `path` as `snapshot` saw it; none when it changed
/// too recently to be told apart from a change within the same tick.
pub(crate) fn of_file(
    path: &std::path::Path,
    snapshot: &fastctx_platform::file_read::Snapshot,
    max: u32,
) -> Option<String> {
    let settled = SystemTime::now().duration_since(snapshot.modified).is_ok_and(|since| since >= SETTLED);
    settled.then(|| format!("file {} {} {max}", path.display(), snapshot.fingerprint()))
}

pub(crate) fn get(key: &str) -> Option<Thumbnail> {
    let mut cache = CACHE.lock().expect("never poisoned");
    cache.tick += 1;
    let tick = cache.tick;
    let (used, thumbnail) = cache.entries.get_mut(key)?;
    *used = tick;
    Some(thumbnail.clone())
}

pub(crate) fn put(key: String, thumbnail: &Thumbnail) {
    let size = thumbnail.base64.len() + thumbnail.mime.len() + key.len();
    if size > BYTES / 8 {
        return;
    }
    let mut cache = CACHE.lock().expect("never poisoned");
    cache.tick += 1;
    let tick = cache.tick;
    if let Some((_, old)) = cache.entries.insert(key.clone(), (tick, thumbnail.clone())) {
        cache.bytes -= old.base64.len() + old.mime.len() + key.len();
    }
    cache.bytes += size;
    while cache.bytes > BYTES {
        let Some(oldest) = cache.entries.iter().min_by_key(|(_, (used, _))| *used).map(|(key, _)| key.clone()) else {
            break;
        };
        if let Some((_, gone)) = cache.entries.remove(&oldest) {
            cache.bytes -= gone.base64.len() + gone.mime.len() + oldest.len();
        }
    }
}
