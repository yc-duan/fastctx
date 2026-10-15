//! Process discovery by an explicitly supplied set of product image paths.
//! This never infers ownership from a process name or a parent PID.

use fastctx_types::ProcessIdentity;
use std::io;
use std::path::{Path, PathBuf};

fn matches(image: &Path, owned: &[PathBuf]) -> bool {
    let spelling = |path: &Path| {
        let path = path.to_string_lossy().replace('\\', "/");
        if cfg!(windows) { path.trim_start_matches("//?/").to_lowercase() } else { path }
    };
    owned.iter().any(|path| spelling(path) == spelling(image))
}

#[cfg(windows)]
pub fn processes(owned: &[PathBuf]) -> io::Result<Vec<ProcessIdentity>> {
    use std::os::windows::ffi::OsStringExt as _;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    let mut found = Vec::new();
    // SAFETY: every handle is owned here and closed, all structures carry
    // their size, and QueryFullProcessImageName receives its actual buffer.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let result = (|| {
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snapshot, &mut entry);
            while more != 0 {
                let length = entry.szExeFile.iter().position(|unit| *unit == 0).unwrap_or(entry.szExeFile.len());
                let name = std::ffi::OsString::from_wide(&entry.szExeFile[..length]);
                // Filter candidates before querying protected system processes. The
                // full image path below still supplies the ownership evidence.
                let candidate = owned
                    .iter()
                    .filter_map(|path| path.file_name())
                    .any(|owned| owned.to_string_lossy().eq_ignore_ascii_case(&name.to_string_lossy()));
                if entry.th32ProcessID != 0 && candidate {
                    let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.th32ProcessID);
                    if process.is_null() {
                        let error = io::Error::last_os_error();
                        if !matches!(error.raw_os_error(), Some(code) if code == ERROR_ACCESS_DENIED as i32 || code == ERROR_INVALID_PARAMETER as i32)
                        {
                            return Err(error);
                        }
                    } else {
                        let queried = (|| {
                            // Win32's native path contract permits 32,767 UTF-16 units.
                            let mut path = vec![0u16; 32768];
                            let mut length = path.len() as u32;
                            if QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) == 0 {
                                return Err(io::Error::last_os_error());
                            }
                            let image = PathBuf::from(std::ffi::OsString::from_wide(&path[..length as usize]));
                            if matches(&image, owned) {
                                found.push(crate::identity::of_handle(process, entry.th32ProcessID)?);
                            }
                            Ok(())
                        })();
                        CloseHandle(process);
                        queried?;
                    }
                }
                more = Process32NextW(snapshot, &mut entry);
            }
            Ok(found)
        })();
        CloseHandle(snapshot);
        result
    }
}

#[cfg(target_os = "macos")]
pub fn processes(owned: &[PathBuf]) -> io::Result<Vec<ProcessIdentity>> {
    let mut found = Vec::new();
    for process in crate::darwin_proc::all()? {
        if process.zombie {
            continue;
        }
        if let Some(image) = crate::darwin_proc::image(process.pid)
            && matches(&image, owned)
            && let Some(identity) = crate::identity_of(process.pid)?
        {
            found.push(identity);
        }
    }
    Ok(found)
}

#[cfg(target_os = "linux")]
pub fn processes(owned: &[PathBuf]) -> io::Result<Vec<ProcessIdentity>> {
    let mut found = Vec::new();
    for process in crate::procfs::all()? {
        if process.is_ending() {
            continue;
        }
        let image = match std::fs::read_link(format!("/proc/{}/exe", process.pid)) {
            Ok(image) => image,
            Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied) => continue,
            Err(error) => return Err(error),
        };
        if matches(&image, owned)
            && let Some(identity) = crate::identity_of(process.pid)?
        {
            found.push(identity);
        }
    }
    Ok(found)
}

/// Whether the process serves a hub (`fastctx hub serve`). A hub on Linux runs
/// the same image as the node, and is stopped with its own service, never
/// swept with the node's processes.
#[cfg(target_os = "linux")]
pub fn serves_hub(process: &ProcessIdentity) -> io::Result<bool> {
    let line = match std::fs::read(format!("/proc/{}/cmdline", process.pid)) {
        Ok(line) => line,
        Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied) => {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    let mut arguments = line.split(|byte| *byte == 0).skip(1);
    Ok(arguments.next() == Some(b"hub".as_slice()) && arguments.next() == Some(b"serve".as_slice()))
}

/// On macOS a hub runs the node's image too; its arguments say which it is.
#[cfg(target_os = "macos")]
pub fn serves_hub(process: &ProcessIdentity) -> io::Result<bool> {
    let output = std::process::Command::new("ps")
        .args(["-o", "args=", "-p", &process.pid.to_string()])
        .env("LC_ALL", "C")
        .output()?;
    let line = String::from_utf8_lossy(&output.stdout);
    let mut words = line.split_whitespace().skip(1);
    Ok(words.next() == Some("hub") && words.next() == Some("serve"))
}

/// A hub on Windows runs as its guardian's payload, an image no node uses.
#[cfg(windows)]
pub fn serves_hub(_process: &ProcessIdentity) -> io::Result<bool> {
    Ok(false)
}
