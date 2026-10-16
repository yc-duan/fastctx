//! Bounded child lifetimes; output uses files so a full pipe cannot hide a hang.

use std::io::Read as _;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub static CLEANUP_FAILURES: AtomicUsize = AtomicUsize::new(0);
static NEXT: AtomicUsize = AtomicUsize::new(0);
const STOP_WITHIN: Duration = Duration::from_secs(10);

pub fn failed(error: impl std::fmt::Display) {
    CLEANUP_FAILURES.fetch_add(1, Ordering::SeqCst);
    eprintln!("FAIL child cleanup: {error}");
}

pub fn stop(child: &mut Option<Child>) -> anyhow::Result<()> {
    if let Some(process) = child.as_mut() {
        if let Some(status) = process.try_wait()? {
            let id = process.id();
            *child = None;
            anyhow::bail!("child {id} exited unexpectedly before shutdown: {status}");
        }
        process.kill()?;
        wait(process, STOP_WITHIN)?;
        *child = None;
    }
    Ok(())
}

fn wait(child: &mut Child, within: Duration) -> anyhow::Result<ExitStatus> {
    let began = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        anyhow::ensure!(began.elapsed() < within, "child {} did not exit within {} s", child.id(), within.as_secs());
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct Owned(Option<Child>);

impl Drop for Owned {
    fn drop(&mut self) {
        if let Err(error) = stop(&mut self.0) {
            failed(error);
        }
    }
}

pub fn run(mut command: Command, input: Option<&[u8]>, within: Duration) -> anyhow::Result<crate::world::Said> {
    let home = command
        .get_envs()
        .find_map(|(key, value)| (key == "FASTCTX_HOME").then(|| value.map(PathBuf::from)).flatten())
        .ok_or_else(|| anyhow::anyhow!("a drill command must name its private home"))?;
    std::fs::create_dir_all(&home)?;
    let serial = NEXT.fetch_add(1, Ordering::SeqCst);
    let path = |kind| home.join(format!("drill-command-{serial}.{kind}"));
    let out = path("out");
    let err = path("err");
    let stdin = if let Some(input) = input {
        let path = path("in");
        std::fs::write(&path, input)?;
        Stdio::from(std::fs::File::open(path)?)
    } else {
        Stdio::null()
    };
    let mut child = Owned(Some(
        command.stdin(stdin).stdout(std::fs::File::create(&out)?).stderr(std::fs::File::create(&err)?).spawn()?,
    ));
    let status = wait(child.0.as_mut().expect("spawned"), within)?;
    child.0 = None;
    let read = |path: &std::path::Path| -> anyhow::Result<String> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        anyhow::ensure!(bytes.len() <= 8 * 1024 * 1024, "command output exceeds 8 MiB: {}", path.display());
        Ok(String::from_utf8_lossy(&bytes).to_string())
    };
    Ok(crate::world::Said { status, out: read(&out)?, err: read(&err)? })
}
