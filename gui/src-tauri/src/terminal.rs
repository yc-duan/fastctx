//! A resident on this computer opened in its own official interface, in a
//! terminal window of its own: `fastctx attach`, run from the copy of
//! fastctx placed for the hosts, exactly as the command line does it.

use std::path::Path;

use fastctx_types::ResidentId;

/// Runs `fastctx attach <resident>` in a new terminal window and waits for
/// it to end; says how it ended when that was not well.
pub fn attach(program: &Path, resident: &ResidentId) -> Result<(), String> {
    let status = spawn(program, resident)?.wait().map_err(|error| error.to_string())?;
    if status.success() {
        return Ok(());
    }
    Err(match status.code() {
        Some(code) => format!("fastctx attach ended with exit {code}"),
        None => format!("fastctx attach ended: {status}"),
    })
}

#[cfg(windows)]
fn spawn(program: &Path, resident: &ResidentId) -> Result<std::process::Child, String> {
    use std::os::windows::process::CommandExt;
    // A console of its own: Windows shows it in the terminal the user chose
    // as default, which keeps the window open on a failing exit.
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    std::process::Command::new(program)
        .arg("attach")
        .arg(&**resident)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map_err(|error| format!("{}: {error}", program.display()))
}

#[cfg(not(windows))]
fn spawn(_program: &Path, _resident: &ResidentId) -> Result<std::process::Child, String> {
    Err("opening a terminal window is made for Windows so far".into())
}
