//! A short host-management command whose owner keeps stdin open.
//! The independent guard survives that owner's exit long enough to stop the
//! command's owned process group/job. The worker preserves native CLI escaping.

use anyhow::{Context as _, bail};
use std::{
    ffi::OsString,
    io::Read as _,
    process::Stdio,
    sync::{Arc, mpsc},
    time::Duration,
};

#[derive(clap::Args)]
pub struct Args {
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..))]
    seconds: u64,
    #[arg(long, hide = true)]
    execute: bool,
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<OsString>,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    if args.execute {
        let mut command = std::process::Command::new(&args.command[0]);
        command.args(&args.command[1..]).stdin(Stdio::null()).stdout(Stdio::inherit()).stderr(Stdio::inherit());
        // Only the two helpers need the node's private World location.
        command.env_remove("FASTCTX_HOME");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(0x0800_0000);
        }
        let status = command.status().context("running guarded host command")?;
        if status.success() {
            return Ok(());
        }
        bail!("host command ended with {status}");
    }
    enum End {
        OwnerGone,
        Command(std::io::Result<std::process::ExitStatus>),
    }
    let (ended, ending) = mpsc::channel();
    let owner = ended.clone();
    std::thread::spawn(move || {
        let mut byte = [0];
        let mut input = std::io::stdin().lock();
        while matches!(input.read(&mut byte), Ok(1..)) {}
        let _ = owner.send(End::OwnerGone);
    });
    let mut worker = vec![OsString::from("command-guard"), "--execute".into(), "--".into()];
    worker.extend(args.command);
    let references = worker.iter().map(OsString::as_os_str).collect::<Vec<_>>();
    let environment = std::env::vars_os().collect::<Vec<_>>();
    let mut child = fastctx_platform::Child::spawn(
        &std::env::current_exe()?,
        &references,
        &std::env::current_dir()?,
        &environment,
    )?;
    drop(child.stdin.take());
    let mut stdout = child.stdout.take().expect("owned stdout");
    let mut stderr = child.stderr.take().expect("owned stderr");
    let child = Arc::new(child);
    let observed = child.clone();
    std::thread::spawn(move || {
        let _ = ended.send(End::Command(observed.wait()));
    });
    std::thread::scope(|scope| -> anyhow::Result<()> {
        let out = scope.spawn(move || std::io::copy(&mut stdout, &mut std::io::stdout()));
        let err = scope.spawn(move || std::io::copy(&mut stderr, &mut std::io::stderr()));
        let result = ending.recv_timeout(Duration::from_secs(args.seconds));
        child.kill()?;
        out.join().map_err(|_| anyhow::anyhow!("host stdout reader stopped"))??;
        err.join().map_err(|_| anyhow::anyhow!("host stderr reader stopped"))??;
        match result {
            Ok(End::Command(status)) => {
                if status?.success() {
                    Ok(())
                } else {
                    bail!("guarded host command failed")
                }
            }
            Ok(End::OwnerGone) => bail!("the host command's owner disconnected"),
            Err(_) => bail!("host command did not finish within {} s", args.seconds),
        }
    })
}
