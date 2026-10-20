//! Batch-file launch inside an inherited node-owned Windows job.

pub(crate) fn run(args: Vec<std::ffi::OsString>) -> anyhow::Result<()> {
    let (program, args) = args.split_first().ok_or_else(|| anyhow::anyhow!("an MCP program is required"))?;
    let mut command = std::process::Command::new(program);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x08000000);
    }
    let status = command.status()?;
    std::process::exit(status.code().unwrap_or(1));
}
