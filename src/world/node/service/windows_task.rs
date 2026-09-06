//! Task definitions and exact process handles for persistent, windowless node services.

use crate::control::paths::ControlPaths;
use crate::world::WorldPaths;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::path::Path;
use windows_sys::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::WaitForSingleObject;

/// Native scheduled-task diagnostics use the Windows OEM code page when stdout is a pipe.
pub(super) fn command_text(bytes: &[u8]) -> Result<String, String> {
    use windows_sys::Win32::Globalization::{GetOEMCP, MB_ERR_INVALID_CHARS, MultiByteToWideChar};
    let length = i32::try_from(bytes.len())
        .map_err(|_| "Service-manager output is too large to decode.".to_string())?;
    // SAFETY: the input span is valid; the first call only measures the required output.
    let page = unsafe { GetOEMCP() };
    let count = unsafe {
        MultiByteToWideChar(
            page,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            length,
            std::ptr::null_mut(),
            0,
        )
    };
    if count == 0 {
        return Err(format!(
            "Cannot decode service-manager output in Windows code page {page}: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut wide = vec![0_u16; count as usize];
    // SAFETY: the output buffer has the capacity returned for this exact input and code page.
    if unsafe {
        MultiByteToWideChar(
            page,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            length,
            wide.as_mut_ptr(),
            count,
        )
    } != count
    {
        return Err(format!(
            "Cannot decode service-manager output: {}",
            std::io::Error::last_os_error()
        ));
    }
    String::from_utf16(&wide)
        .map(|text| text.trim().to_string())
        .map_err(|error| format!("Cannot decode service-manager output: {error}"))
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn path_text(path: &Path) -> Result<&str, String> {
    path.to_str()
        .filter(|text| !text.chars().any(char::is_control) && !text.contains('"'))
        .ok_or_else(|| {
            format!(
                "Task Scheduler cannot represent the path {}.",
                crate::paths::display_path(path)
            )
        })
}

fn quoted_path(path: &Path) -> Result<String, String> {
    let text = path_text(path)?;
    let trailing = text.chars().rev().take_while(|ch| *ch == '\\').count();
    Ok(format!("\"{text}{}\"", "\\".repeat(trailing)))
}

pub(super) fn definition(
    sid: &str,
    host: &Path,
    binary: &Path,
    home: &Path,
) -> Result<String, String> {
    let service_account = matches!(sid, "S-1-5-18" | "S-1-5-19" | "S-1-5-20");
    let arguments = format!(
        "--headless {} node run --service-host --home {}",
        quoted_path(binary)?,
        quoted_path(home)?
    );
    let host = xml(path_text(host)?);
    let arguments = xml(&arguments);
    let sid = xml(sid);
    // ServiceAccount is a COM enum, not an XML value; its SID implies that logon (2026-09-06).
    let logon = if service_account {
        ""
    } else {
        "<LogonType>InteractiveToken</LogonType>"
    };
    let trigger = if service_account {
        "<BootTrigger><Enabled>true</Enabled></BootTrigger>".to_string()
    } else {
        format!("<LogonTrigger><Enabled>true</Enabled><UserId>{sid}</UserId></LogonTrigger>")
    };
    // Scheduler defaults stop tasks on battery and after 72 hours; a node is a service.
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Triggers>{trigger}</Triggers>
  <Principals><Principal id="Node"><UserId>{sid}</UserId>{logon}<RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate><StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand><Enabled>true</Enabled>
    <RunOnlyIfIdle>false</RunOnlyIfIdle><WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <RestartOnFailure><Interval>PT1M</Interval><Count>3</Count></RestartOnFailure>
  </Settings>
  <Actions Context="Node"><Exec><Command>{host}</Command><Arguments>{arguments}</Arguments></Exec></Actions>
</Task>
"#
    ))
}

/// Captures before ending the task so PID reuse cannot change which process we wait for.
pub(super) fn capture_node(paths: &ControlPaths) -> Result<Option<OwnedHandle>, String> {
    let Some(status) = crate::world::node::read_status(&WorldPaths::from_control(paths))? else {
        return Ok(None);
    };
    let identity = if let Some(identity) = status.process_identity {
        if identity.pid != status.pid {
            return Err("The node status has inconsistent process identities.".into());
        }
        identity
    } else {
        let Some(identity) = crate::process_identity::process_identity(status.pid) else {
            return Ok(None);
        };
        let started = crate::world::parse_rfc3339(&status.started_at)?;
        let ticks = identity
            .started
            .parse::<i128>()
            .map_err(|error| format!("Cannot inspect the legacy node process identity: {error}"))?;
        // Legacy status timestamps have second precision; a newer process is not that node.
        if ticks > started.unix_timestamp_nanos() / 100 + 116_444_736_000_000_000 + 10_000_000 {
            return Ok(None);
        }
        identity
    };
    crate::world::node::service_lifetime::process_handle(&identity)
}

pub(super) fn wait_for_stop(process: Option<OwnedHandle>) -> Result<(), String> {
    let Some(process) = process else {
        return Ok(());
    };
    // SAFETY: the owned process handle grants synchronization rights and stays live.
    match unsafe { WaitForSingleObject(process.as_raw_handle(), 15_000) } {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_FAILED => Err(format!("Cannot confirm node shutdown: {}", std::io::Error::last_os_error())),
        _ => Err("The task stopped, but its node did not exit within 15 seconds. Check the node log before restarting it.".into()),
    }
}
