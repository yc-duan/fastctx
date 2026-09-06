//! Windows logon tasks are scoped to an OS account and one enrollment HOME.

use super::{run_checked, service_binary, windows_task};
use crate::control::paths::ControlPaths;
use crate::process_policy::noninteractive_command;
use crate::world::node::service_identity;

const LEGACY_TASK: &str = "FastCtx Node";

fn console_host() -> Result<std::path::PathBuf, String> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

    let mut buffer = vec![0_u16; 32_768];
    // SAFETY: The buffer is writable for the supplied number of UTF-16 code units.
    let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if length == 0 || length as usize >= buffer.len() {
        return Err(format!(
            "Cannot locate the Windows console host: {}",
            std::io::Error::last_os_error()
        ));
    }
    let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length as usize]))
        .join("conhost.exe");
    if !path.is_file() {
        return Err("The Windows headless console host is unavailable.".into());
    }
    Ok(path)
}

fn task_name(paths: &ControlPaths, user: Option<&str>) -> Result<String, String> {
    Ok(format!(
        "FastCtx Node {}",
        service_identity::scope(paths, user)?
    ))
}
fn listing() -> Result<String, String> {
    let mut command = noninteractive_command("schtasks");
    command.args(["/Query", "/FO", "CSV", "/NH"]);
    run_checked(command, "query the user's scheduled tasks")
}
fn listed(list: &str, name: &str) -> bool {
    list.lines()
        .any(|line| line.starts_with(&format!("\"\\{name}\",")))
}
fn xml(name: &str) -> Result<String, String> {
    let mut command = noninteractive_command("schtasks");
    command.args(["/Query", "/TN", name, "/XML"]);
    run_checked(command, "read the node task registration")
}
fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    xml.split_once(&format!("<{name}>"))?
        .1
        .split_once(&format!("</{name}>"))
        .map(|(value, _)| value)
}
fn owned(name: &str, sid: &str) -> Result<bool, String> {
    let xml = xml(name)?;
    let owner = tag(&xml, "UserId")
        .ok_or_else(|| "The node task does not identify its Windows account.".to_string())?;
    let owner = if owner.starts_with("S-1-") {
        owner.to_string()
    } else {
        service_identity::windows_sid(Some(owner))?
    };
    Ok(owner.eq_ignore_ascii_case(sid))
}
fn registered(paths: &ControlPaths) -> Result<Option<String>, String> {
    let list = listing()?;
    let name = task_name(paths, None)?;
    let sid = service_identity::windows_sid(None)?;
    if listed(&list, &name) {
        return if owned(&name, &sid)? {
            Ok(Some(name))
        } else {
            Err("Permission denied: this node task belongs to another Windows account.".into())
        };
    }
    if listed(&list, LEGACY_TASK)
        && service_identity::is_default_home(paths)?
        && owned(LEGACY_TASK, &sid)?
    {
        let definition = xml(LEGACY_TASK)?;
        if tag(&definition, "Arguments").is_some_and(|args| args.trim() == "node run") {
            return Ok(Some(LEGACY_TASK.into()));
        }
    }
    Ok(None)
}
fn stop_task(name: &str, paths: &ControlPaths) -> Result<(), String> {
    let process = windows_task::capture_node(paths);
    let mut command = noninteractive_command("schtasks");
    command.args(["/End", "/TN", name]);
    run_checked(command, "stop the node logon task")?;
    windows_task::wait_for_stop(process.map_err(|error| {
        format!("The task stopped, but node shutdown could not be confirmed: {error}")
    })?)
}
fn delete_task(name: &str) -> Result<(), String> {
    let mut command = noninteractive_command("schtasks");
    command.args(["/Delete", "/TN", name, "/F"]);
    run_checked(command, "delete the node logon task").map(|_| ())
}
fn start_task(name: &str) -> Result<String, String> {
    let mut command = noninteractive_command("schtasks");
    command.args(["/Run", "/TN", name]);
    run_checked(command, "start the node logon task")?;
    Ok(format!("Started logon task \"{name}\"."))
}

pub(super) fn inspect(paths: &ControlPaths) -> Result<bool, String> {
    registered(paths).map(|name| name.is_some())
}
pub(super) fn install(paths: &ControlPaths, user: Option<&str>) -> Result<String, String> {
    let host = console_host()?;
    let binary = service_binary(paths)?;
    let name = task_name(paths, user)?;
    let sid = service_identity::windows_sid(user)?;
    let definition = windows_task::definition(&sid, &host, &binary, &paths.home)?;
    let mut task_file = tempfile::NamedTempFile::new()
        .map_err(|error| format!("Cannot stage the node task definition: {error}"))?;
    let encoded = std::iter::once(0xfeff_u16)
        .chain(definition.encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    std::io::Write::write_all(&mut task_file, &encoded)
        .map_err(|error| format!("Cannot write the node task definition: {error}"))?;
    std::io::Write::flush(&mut task_file)
        .map_err(|error| format!("Cannot finish the node task definition: {error}"))?;
    let task_file = task_file.into_temp_path();
    let existing = if user.is_none() {
        registered(paths)?
    } else {
        let list = listing()?;
        if listed(&list, &name) {
            if !owned(&name, &sid)? {
                return Err(
                    "Permission denied: this node task belongs to another Windows account.".into(),
                );
            }
            Some(name.clone())
        } else {
            None
        }
    };
    if let Some(existing) = existing {
        stop_task(&existing, paths)?;
        if existing != name {
            delete_task(&existing)?;
        }
    }
    let mut create = noninteractive_command("schtasks");
    create
        .args(["/Create", "/TN", &name, "/F", "/XML"])
        .arg(task_file.as_os_str());
    run_checked(create, "register the node logon task")?;
    start_task(&name)?;
    Ok(format!(
        "Registered and started \"{name}\" for HOME {}.",
        crate::paths::display_path(&paths.home)
    ))
}
pub(super) fn start(paths: &ControlPaths) -> Result<String, String> {
    start_task(
        &registered(paths)?
            .ok_or_else(|| "No node service is registered for this HOME.".to_string())?,
    )
}
pub(super) fn stop(paths: &ControlPaths) -> Result<String, String> {
    let Some(name) = registered(paths)? else {
        return Ok("No node service is registered for this HOME.".into());
    };
    stop_task(&name, paths)?;
    Ok(format!("Stopped \"{name}\"."))
}
pub(super) fn uninstall(paths: &ControlPaths) -> Result<String, String> {
    let Some(name) = registered(paths)? else {
        return Ok("No node service is registered for this HOME.".into());
    };
    stop_task(&name, paths)?;
    delete_task(&name)?;
    Ok(format!("Removed \"{name}\"."))
}
