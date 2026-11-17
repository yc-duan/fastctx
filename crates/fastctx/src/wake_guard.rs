//! A resume CLI is owned; the background daemon it may create is not.

use anyhow::{Context as _, bail};
use std::{
    ffi::OsString,
    io::Read as _,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

pub(crate) fn run(seconds: u64, command: Vec<OsString>) -> anyhow::Result<()> {
    let (ended, owner) = mpsc::channel();
    std::thread::spawn(move || {
        let mut byte = [0];
        while matches!(std::io::stdin().read(&mut byte), Ok(1..)) {}
        let _ = ended.send(());
    });
    let mut process = Command::new(&command[0]);
    process
        .args(&command[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .env_remove("FASTCTX_HOME");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        process.creation_flags(0x0800_0000);
    }
    let mut child = process.spawn().context("starting Claude wake launcher")?;
    let began = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            bail!("Claude wake launcher ended with {status}");
        }
        let gone = owner.recv_timeout(Duration::from_millis(20)).is_ok();
        if gone || began.elapsed() >= Duration::from_secs(seconds) {
            // Only this CLI is ours. A daemon detached from it owns the
            // accepted session and must survive a successful resume.
            let _ = child.kill();
            let _ = child.wait();
            if gone {
                bail!("the Claude wake launcher's owner disconnected")
            }
            bail!("Claude wake launcher did not finish within {seconds} s");
        }
    }
}
