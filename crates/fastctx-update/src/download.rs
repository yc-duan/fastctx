//! A package downloaded from where its release published it, going on from
//! what an earlier download left.

use std::io::{Seek as _, Write as _};
use std::path::Path;

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
