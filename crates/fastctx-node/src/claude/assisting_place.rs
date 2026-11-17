//! Where a session the World starts for another account runs, in the login
//! that lends the agent (`host-integration-route`).
//!
//! Such a session reads none of the login's own settings: its launch names
//! only the `local` source, so no hook, permission or server of the lender's
//! reaches it. Claude Code starts a background session without approvals
//! only where an enabled source records that the user accepted that mode
//! (2.1.322); neither `--settings` nor a project's settings count, the
//! project-local settings of the directory it starts in do. So each login
//! has its own directory under the World, whose local settings say the user
//! accepted exactly while the login's own settings say so, and nothing else.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// What the local settings say where the login accepted.
const ACCEPTED: &str = "{\n  \"skipDangerousModePermissionPrompt\": true\n}\n";

/// The directory an assisting session of the login `env` names (its
/// `CLAUDE_CONFIG_DIR`, or the default home) starts in, its local settings
/// brought in line with the login's word first.
pub(super) fn place(world: &Path, env: &[(OsString, OsString)], user_home: &Path) -> io::Result<PathBuf> {
    let home = crate::homes::claude_default(env, user_home);
    let key = Sha256::digest(home.to_string_lossy().as_bytes());
    let directory = world.join("claude-assisting").join(hex(&key[..8]));
    let settings = directory.join(".claude").join("settings.local.json");
    let accepted = match fastctx_platform::disk::read(home.join("settings.json")) {
        Ok(bytes) => fastctx_install::claude::consent::accepted(Some(&bytes)).unwrap_or(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    fastctx_platform::ensure_private_dir(&directory)?;
    if accepted {
        fastctx_platform::ensure_private_dir(&directory.join(".claude"))?;
        if fastctx_platform::disk::read(&settings).ok().as_deref() != Some(ACCEPTED.as_bytes()) {
            fastctx_platform::disk::write(&settings, ACCEPTED)?;
        }
    } else {
        match fastctx_platform::disk::remove_file(&settings) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    Ok(directory)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
