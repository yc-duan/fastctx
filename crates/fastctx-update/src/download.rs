//! A package downloaded from where its release published it, going on from
//! what an earlier download left.

use std::io::{Seek as _, Write as _};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::pace::{Pace, TRIES};

/// Downloads `url` into `file` as `from_url` does, connecting again when a
/// connection fails or breaks and going on from what came, and giving up
/// once `TRIES` connections running brought nothing (`crate::pace`). Its
/// speed, in bytes a second, when bytes came.
pub async fn patiently(
    url: &str,
    file: &Path,
    progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
) -> Result<Option<u64>, String> {
    let mut pace = Pace::new(Instant::now());
    let mut failed = 0;
    let mut wait = Duration::from_secs(1);
    loop {
        let before = length(file);
        let result = {
            let mut heard = |brought, total| {
                pace.heard(Instant::now(), brought);
                progress(brought, total);
            };
            from_url(url, file, &mut heard).await
        };
        match result {
            Ok(()) => return Ok(pace.speed()),
            Err(why) => {
                // One that brought bytes did connect: only a run of none counts.
                failed = if length(file) > before { 0 } else { failed + 1 };
                if failed >= TRIES {
                    return Err(format!("{why}; {TRIES} connections running brought nothing"));
                }
            }
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(Duration::from_secs(8));
    }
}

fn length(file: &Path) -> u64 {
    std::fs::metadata(file).map(|metadata| metadata.len()).unwrap_or(0)
}

/// Downloads `url` into `file`, appending to what is there when the server
/// sends the rest, starting again when it sends the whole. `progress` hears
/// the bytes in the file and the whole size as they grow.
pub async fn from_url(
    url: &str,
    file: &Path,
    progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
) -> Result<(), String> {
    // Written at the end it reaches, and cut back when the whole comes again:
    // a file opened only to append cannot be cut on Windows.
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(file)
        .map_err(|error| format!("{}: {error}", file.display()))?;
    let have = out.seek(std::io::SeekFrom::End(0)).map_err(|error| error.to_string())?;
    let mut response = fastctx_net::web::get(url, "application/octet-stream", have).await?;
    let (mut written, total) = match response.status {
        206 => (have, response.length.map(|rest| rest + have)),
        200 => {
            out.set_len(0).map_err(|error| error.to_string())?;
            out.seek(std::io::SeekFrom::Start(0)).map_err(|error| error.to_string())?;
            (0, response.length)
        }
        // Everything was there already.
        416 if have > 0 => {
            progress(have, Some(have));
            return Ok(());
        }
        status => return Err(format!("{} answered {status}", response.url)),
    };
    progress(written, total);
    while let Some(chunk) = response.chunk().await? {
        out.write_all(&chunk).map_err(|error| format!("{}: {error}", file.display()))?;
        written += chunk.len() as u64;
        progress(written, total);
    }
    if let Some(total) = total
        && written != total
    {
        return Err(format!("{} ended after {written} of {total} bytes", response.url));
    }
    out.sync_all().map_err(|error| error.to_string())
}
