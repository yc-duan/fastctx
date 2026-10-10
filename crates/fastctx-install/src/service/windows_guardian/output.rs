//! A child inherits a pipe; one rolling writer owns the diagnostic file.

use super::*;
use std::io::BufRead as _;

pub(super) fn start(
    reader: PipeReader,
    log: Arc<fastctx_platform::rolling_log::RollingLog>,
    token: Option<OwnedHandle>,
) -> io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("guardian child diagnostics".into()).spawn(move || {
        let result = (|| {
            // Rotations of a user's paths retain that user's authority.
            let _user = Impersonation::new(token.as_ref().map(|token| token.as_raw_handle())).map_err(io::Error::other)?;
            let mut reader = std::io::BufReader::new(reader);
            let mut failed = false;
            loop {
                let bytes = reader.fill_buf()?;
                if bytes.is_empty() { break; }
                match log.write(bytes) {
                    Ok(()) => {
                        if failed { report("fastctx service diagnostics resumed"); }
                        failed = false;
                    },
                    Err(error) => {
                        if !failed { report(&format!("fastctx service diagnostics cannot be kept: {error}; the pipe remains drained until logging resumes")); }
                        failed = true;
                    },
                }
                let consumed = bytes.len();
                reader.consume(consumed);
            }
            Ok::<_, io::Error>(())
        })();
        if let Err(error) = result {
            report(&format!("fastctx service diagnostics failed: {error}"));
        }
    })
}

fn report(text: &str) {
    let message: Vec<u16> = format!("{text}\n").encode_utf16().chain([0]).collect();
    // SAFETY: the string is NUL-terminated and lives through the call.
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringW(message.as_ptr());
    }
}
