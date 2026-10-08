//! The handover from the upgrader of an old fastctx (0.1.0 to 0.2.6). Having
//! installed this version, it starts it with `FASTCTX_UPDATE_FINALIZE`
//! naming a request, and rolls the install back unless a health file the
//! request names appears holding exactly `ok\n` (`world-upgrade.md`). The
//! requests of all nine versions were read off their own runs: schema 1
//! (0.1.0), 2 (0.1.1) and 3 (0.2.0 to 0.2.6), each naming `health_file`.

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context as _, bail};

const FINALIZE: &str = "FASTCTX_UPDATE_FINALIZE";

#[derive(serde::Deserialize)]
struct Request {
    schema_version: u32,
    health_file: PathBuf,
}

/// Answers an old upgrader that started this program, when one did. The
/// variable leaves this process's environment first, so nothing started
/// from here inherits the handover. A request this version cannot read is
/// answered with nothing, and the upgrader puts the old version back.
pub fn finalize() -> anyhow::Result<()> {
    let Some(request) = std::env::var_os(FINALIZE) else { return Ok(()) };
    // SAFETY: called first in `main`, before any other thread exists.
    unsafe { std::env::remove_var(FINALIZE) };
    let path = fastctx_platform::local_path(PathBuf::from(request))?;
    let bytes = std::fs::read(&path).with_context(|| format!("reading the upgrade request {}", path.display()))?;
    let mut request: Request =
        serde_json::from_slice(&bytes).with_context(|| format!("reading the upgrade request {}", path.display()))?;
    if !(1..=3).contains(&request.schema_version) {
        bail!(
            "{}: upgrade request schema {} is not one this version reads (1 to 3)",
            path.display(),
            request.schema_version
        );
    }
    request.health_file = fastctx_platform::local_path(request.health_file)?;
    // Created afresh, as the upgrader expects: a file already there was not
    // written by this start.
    let mut health = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&request.health_file)
        .with_context(|| format!("creating {}", request.health_file.display()))?;
    health.write_all(b"ok\n")?;
    health.sync_all().with_context(|| format!("writing {}", request.health_file.display()))?;
    Ok(())
}
