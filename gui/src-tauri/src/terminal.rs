//! A resident on this computer opened in its own official interface, in a
//! terminal window of its own: `fastctx attach`, run from the copy of
//! fastctx placed for the hosts, exactly as the command line does it.

use std::path::Path;

use fastctx_types::ResidentId;

/// Runs `fastctx attach <resident>` in a new terminal window and waits for
/// it to end where the terminal lets it be waited for; says how it ended
/// when that was not well.
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

/// Terminal.app runs a `.command` file it is given in a window of its own,
/// with no permission to script it asked for; the file removes itself as it
/// starts. `open` returns once Terminal has it, and the window stays with
/// whatever the attach printed once it ends.
#[cfg(target_os = "macos")]
fn spawn(program: &Path, resident: &ResidentId) -> Result<std::process::Child, String> {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;
    let quote = |word: &str| format!("'{}'", word.replace('\'', r"'\''"));
    let mut file = tempfile::Builder::new()
        .prefix("fastctx-attach-")
        .suffix(".command")
        .tempfile()
        .map_err(|error| error.to_string())?;
    write!(file, "#!/bin/sh\nrm -f -- \"$0\"\nexec {} attach {}\n", quote(&program.to_string_lossy()), quote(resident))
        .map_err(|error| error.to_string())?;
    file.as_file().set_permissions(std::fs::Permissions::from_mode(0o700)).map_err(|error| error.to_string())?;
    let (_, path) = file.keep().map_err(|error| error.to_string())?;
    std::process::Command::new("/usr/bin/open")
        .args(["-a", "Terminal"])
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|error| format!("opening Terminal: {error}"))
}

/// The desktop's own terminal: Debian and Ubuntu name it
/// `x-terminal-emulator`; otherwise the first of the common ones found.
#[cfg(all(unix, not(target_os = "macos")))]
fn spawn(program: &Path, resident: &ResidentId) -> Result<std::process::Child, String> {
    const TERMINALS: [(&str, &[&str]); 6] = [
        ("x-terminal-emulator", &["-e"]),
        ("gnome-terminal", &["--"]),
        ("konsole", &["-e"]),
        ("xfce4-terminal", &["-x"]),
        ("kitty", &[]),
        ("xterm", &["-e"]),
    ];
    for (terminal, lead) in TERMINALS {
        match std::process::Command::new(terminal)
            .args(lead)
            .arg(program)
            .arg("attach")
            .arg(&**resident)
            .stdin(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => return Ok(child),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("{terminal}: {error}")),
        }
    }
    Err("no terminal program was found on this computer".into())
}
