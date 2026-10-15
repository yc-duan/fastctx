//! Rights beyond the user's own: whether this process has them, and, on
//! Windows, running a program with them after the system asks the person at
//! the screen (User Account Control).

use std::ffi::OsString;
use std::io;
use std::path::Path;

/// Whether this process runs with administrator rights: elevated on
/// Windows, as root on Unix.
#[cfg(windows)]
pub fn is_elevated() -> io::Result<bool> {
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TokenElevation};

    crate::windows_user::with_current_token(|token| {
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut length = 0;
        // SAFETY: the buffer is a TOKEN_ELEVATION of the size given.
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenElevation,
                (&raw mut elevation).cast(),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut length,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(elevation.TokenIsElevated != 0)
    })
}

#[cfg(unix)]
pub fn is_elevated() -> io::Result<bool> {
    // SAFETY: geteuid has no preconditions.
    Ok(unsafe { libc::geteuid() } == 0)
}

/// Whether this process acts as an administrator of the computer now: on
/// Windows its token holds the Administrators group enabled, which neither
/// an administrator's token filtered by User Account Control nor one with
/// their rights held back, as a basic user's, does; root on Unix. What only
/// an administrator may override is asked of this.
#[cfg(windows)]
pub fn is_administrator() -> io::Result<bool> {
    use windows_sys::Win32::Security::{
        CheckTokenMembership, CreateWellKnownSid, SECURITY_MAX_SID_SIZE, WinBuiltinAdministratorsSid,
    };

    let mut sid = [0u8; SECURITY_MAX_SID_SIZE as usize];
    let mut size = SECURITY_MAX_SID_SIZE;
    // SAFETY: the buffer holds the largest SID and its size is given.
    if unsafe {
        CreateWellKnownSid(WinBuiltinAdministratorsSid, std::ptr::null_mut(), sid.as_mut_ptr().cast(), &mut size)
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut member = 0;
    // SAFETY: a null token is the calling thread's effective token; the SID
    // was made above and outlives the call.
    if unsafe { CheckTokenMembership(std::ptr::null_mut(), sid.as_mut_ptr().cast(), &mut member) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(member != 0)
}

#[cfg(unix)]
pub fn is_administrator() -> io::Result<bool> {
    is_elevated()
}

/// Runs `program` with `args` as an administrator, once the person at the
/// screen agrees, and waits for it; returns its exit code. Their saying no
/// is `PermissionDenied`.
#[cfg(windows)]
pub fn run_elevated(program: &Path, args: &[OsString]) -> io::Result<u32> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };

    let wide = |text: &std::ffi::OsStr| -> Vec<u16> { text.encode_wide().chain([0]).collect() };
    let verb = wide("runas".as_ref());
    let file = wide(program.as_os_str());
    let parameters = wide(&command_line(args));
    // SAFETY: zero is a valid starting value for every field of the struct.
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    // SW_HIDE: the program says what it did on the caller's own terminal.
    info.nShow = 0;
    // SAFETY: every string outlives the call.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let error = io::Error::last_os_error();
        return Err(if error.raw_os_error() == Some(ERROR_CANCELLED as i32) {
            io::Error::new(io::ErrorKind::PermissionDenied, "the request for administrator rights was declined")
        } else {
            error
        });
    }
    let process = info.hProcess;
    // SAFETY: SEE_MASK_NOCLOSEPROCESS hands this process handle to the caller.
    let waited = unsafe { WaitForSingleObject(process, INFINITE) };
    let mut code = 0;
    let read = unsafe { GetExitCodeProcess(process, &mut code) };
    let error = io::Error::last_os_error();
    unsafe { CloseHandle(process) };
    if waited != WAIT_OBJECT_0 || read == 0 {
        return Err(error);
    }
    Ok(code)
}

/// A command line that splits back into exactly `args`, as programs that
/// read their arguments with the C runtime's rules do.
#[cfg(windows)]
fn command_line(args: &[OsString]) -> OsString {
    let mut line = OsString::new();
    for (index, arg) in args.iter().enumerate() {
        if index > 0 {
            line.push(" ");
        }
        let text = arg.to_string_lossy();
        if !text.is_empty() && !text.contains([' ', '\t', '\n', '\u{b}', '"']) {
            line.push(arg);
            continue;
        }
        let mut quoted = String::from('"');
        let mut backslashes = 0;
        for character in text.chars() {
            match character {
                '\\' => backslashes += 1,
                '"' => {
                    quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                    quoted.push('"');
                    backslashes = 0;
                }
                other => {
                    quoted.push_str(&"\\".repeat(backslashes));
                    quoted.push(other);
                    backslashes = 0;
                }
            }
        }
        quoted.push_str(&"\\".repeat(backslashes * 2));
        quoted.push('"');
        line.push(quoted);
    }
    line
}

/// On macOS the system's own dialog asks for an administrator's password,
/// through `osascript`; declining it reads as PermissionDenied.
#[cfg(target_os = "macos")]
pub fn run_elevated(program: &Path, args: &[OsString]) -> io::Result<u32> {
    // Each word in single quotes for the shell, then the whole line in an
    // AppleScript string.
    let quote = |word: &std::ffi::OsStr| format!("'{}'", word.to_string_lossy().replace('\'', r"'\''"));
    let mut line = quote(program.as_os_str());
    for arg in args {
        line.push(' ');
        line.push_str(&quote(arg));
    }
    let script =
        format!("do shell script \"{}\" with administrator privileges", line.replace('\\', r"\\").replace('"', "\\\""));
    let output = std::process::Command::new("/usr/bin/osascript").args(["-e", &script]).output()?;
    if output.status.success() {
        return Ok(0);
    }
    let said = String::from_utf8_lossy(&output.stderr);
    if said.contains("(-128)") {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the administrator's approval was declined"));
    }
    // A failing command ends the script with its own exit status in brackets.
    let code =
        said.trim_end().strip_suffix(')').and_then(|rest| rest.rsplit('(').next()).and_then(|code| code.parse().ok());
    Ok(code.unwrap_or(1))
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn run_elevated(_program: &Path, _args: &[OsString]) -> io::Result<u32> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "on Unix, administrator rights are asked for with sudo"))
}
