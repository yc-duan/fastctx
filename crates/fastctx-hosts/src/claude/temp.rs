//! Where Claude Code keeps what one session writes to temporary files: the
//! output of its background tasks, its scratchpad, pictures it was given.
//!
//! As 2.1.300 makes it, under the temporary directory (`CLAUDE_CODE_TMPDIR`,
//! else the system's) is `claude-<uid>` on Unix and `claude` on Windows; in
//! it, one directory for each working directory, named by its path with
//! every UTF-16 unit but an ASCII letter or digit made `-`; in that, one for
//! each session. Claude Code removes none of them.

use std::ffi::OsString;
use std::path::PathBuf;

/// The longest name of a working directory kept whole; a longer one is cut
/// and given a hash, which is not worked out here.
const WHOLE: usize = 200;

/// The directory of the session `session`, started in `cwd` with the
/// environment `env`; `None` when it cannot be worked out.
pub fn session_directory(env: &[(OsString, OsString)], cwd: &str, session: &str) -> Option<PathBuf> {
    if session.is_empty() || !session.bytes().all(|byte| byte.is_ascii_hexdigit() || byte == b'-') {
        return None;
    }
    let name: String = char::decode_utf16(cwd.encode_utf16().map(|unit| match u8::try_from(unit) {
        Ok(byte) if byte.is_ascii_alphanumeric() => unit,
        _ => u16::from(b'-'),
    }))
    .collect::<Result<_, _>>()
    .ok()?;
    if name.is_empty() || name.len() > WHOLE {
        return None;
    }
    Some(root(env)?.join(name).join(session))
}

/// `claude-<uid>`, or `claude` on Windows, in the temporary directory.
fn root(env: &[(OsString, OsString)]) -> Option<PathBuf> {
    let get = |name: &str| {
        env.iter()
            .find(|(key, _)| if cfg!(windows) { key.eq_ignore_ascii_case(name) } else { key == name })
            .map(|(_, value)| value.clone())
            .filter(|value| !value.is_empty())
    };
    let base = match get("CLAUDE_CODE_TMPDIR") {
        Some(directory) => PathBuf::from(directory),
        None => system_temp(&get)?,
    };
    Some(match fastctx_platform::user_id() {
        Some(uid) => base.join(format!("claude-{uid}")),
        None => base.join("claude"),
    })
}

/// The temporary directory as Node.js gives it (`os.tmpdir()`).
fn system_temp(get: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if cfg!(windows) {
        get("TEMP")
            .or_else(|| get("TMP"))
            .map(PathBuf::from)
            .or_else(|| get("SystemRoot").or_else(|| get("windir")).map(|root| PathBuf::from(root).join("temp")))
    } else {
        Some(get("TMPDIR").or_else(|| get("TMP")).or_else(|| get("TEMP")).map_or_else(|| "/tmp".into(), PathBuf::from))
    }
}
