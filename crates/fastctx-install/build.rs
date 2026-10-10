//! When this program's source was made, so that two builds reporting the
//! same package version still have an order (`program::newer`): the release
//! scripts give `SOURCE_DATE_EPOCH`; a build from a git checkout takes its
//! commit's time. A build with neither has no time, and its order against
//! another of the same version stays unknown.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let given = std::env::var("SOURCE_DATE_EPOCH").ok().filter(|value| value.trim().parse::<u64>().is_ok());
    if let Some(built) = given.or_else(commit_time) {
        println!("cargo:rustc-env=FASTCTX_BUILT={}", built.trim());
    }
}

fn commit_time() -> Option<String> {
    let dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?);
    let git = |args: &[&str]| -> Option<String> {
        let output = Command::new("git").arg("-C").arg(&dir).args(args).output().ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    // Whatever moves HEAD reruns this: the worktree's HEAD, the branch it
    // names, and the packed refs a branch may live in.
    let mut watched = vec!["HEAD".to_string(), "packed-refs".to_string()];
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watched.push(branch);
    }
    for name in watched {
        if let Some(path) = git(&["rev-parse", "--git-path", &name]) {
            let path = Path::new(&path);
            let path = if path.is_absolute() { path.to_path_buf() } else { dir.join(path) };
            // A path that does not exist would rerun this on every build.
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    git(&["log", "-1", "--format=%ct"]).filter(|time| time.parse::<u64>().is_ok())
}
