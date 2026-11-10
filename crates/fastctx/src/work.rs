//! Worker dispatch happens before the command runtime is started. Crates own
//! static role slices; the executable links those slices explicitly here.

use std::io;

use fastctx_platform::worker::{self, Role};

const BUILTIN: &[Role] = &[Role { name: "echo", serve: echo }];
// Add an owning crate's exported &[Role] here as roles are introduced.
const ROLES: &[&[Role]] = &[BUILTIN];

fn echo(memory: u64) -> io::Result<()> {
    worker::serve(memory, |request| Ok(request.to_vec()))
}

pub fn run() -> anyhow::Result<()> {
    let arguments: Vec<_> = std::env::args_os().skip(2).collect();
    anyhow::ensure!(arguments.len() == 2, "work needs a role and memory allowance");
    let role = arguments[0].to_str().ok_or_else(|| anyhow::anyhow!("invalid worker role"))?;
    let memory = arguments[1].to_str().ok_or_else(|| anyhow::anyhow!("invalid memory allowance"))?.parse()?;
    worker::dispatch(role, memory, ROLES)?;
    Ok(())
}
