//! SCM registrations and unbounded native recovery. The image path always
//! quotes the ProgramData expansion; actual node paths live in protected data.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::ptr;
use std::time::Duration;

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};
use windows_service::service::{
    Service as Registered, ServiceAccess, ServiceAction, ServiceActionType, ServiceFailureActions,
    ServiceFailureResetPeriod, ServiceState,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_sys::Win32::Foundation::{
    ERROR_SERVICE_DOES_NOT_EXIST, ERROR_SERVICE_MARKED_FOR_DELETE, ERROR_SERVICE_NOT_ACTIVE, WAIT_IO_COMPLETION,
    WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION};
use windows_sys::Win32::System::Services::*;
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForSingleObjectEx};

use super::security::{self, Descriptor};

#[derive(Serialize, Deserialize)]
pub(super) struct Snapshot {
    display: Vec<u16>,
    start: u32,
    reset: u32,
    actions: Vec<(i32, u32)>,
    on_error_stop: bool,
    running: bool,
}

pub(super) fn open(name: &str, access: ServiceAccess) -> anyhow::Result<Option<Registered>> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    match manager.open_service(name, access) {
        Ok(service) => Ok(Some(service)),
        Err(windows_service::Error::Winapi(error)) if matches!(error.raw_os_error(), Some(code) if code == ERROR_SERVICE_DOES_NOT_EXIST as i32 || code == ERROR_SERVICE_MARKED_FOR_DELETE as i32) => {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

pub(super) fn command(name: &str) -> String {
    format!("\"%ProgramData%\\fastctx-services\\{name}\\guardian.exe\" service-host {name}")
}

pub(super) fn capture(name: &str) -> anyhow::Result<Option<Snapshot>> {
    let Some(service) = open(name, ServiceAccess::QUERY_CONFIG | ServiceAccess::QUERY_STATUS)? else { return Ok(None) };
    let config = service.query_config()?;
    let mut expected = OsString::from("\"");
    expected.push(security::directory(name)?.join("guardian.exe"));
    expected.push(format!("\" service-host {name}"));
    // QueryServiceConfig expands environment variables. Never feed that
    // expanded path back to SCM: a literal percent in ProgramData would be
    // expanded a second time. Registration/restoration use the fixed macro.
    if config.executable_path.as_os_str() != expected
        || config.account_name.as_deref() != Some(std::ffi::OsStr::new("LocalSystem"))
    {
        bail!("{name} is not this SYSTEM guardian registration");
    }
    let actions = service.get_failure_actions()?;
    if actions.command.as_ref().is_some_and(|s| !s.is_empty())
        || actions.reboot_msg.as_ref().is_some_and(|s| !s.is_empty())
    {
        bail!("{name} has failure actions outside this guardian's contract");
    }
    Ok(Some(Snapshot {
        display: config.display_name.encode_wide().collect(),
        start: config.start_type.to_raw(),
        reset: actions.reset_period.to_raw(),
        actions: actions
            .actions
            .unwrap_or_default()
            .iter()
            .map(|action| {
                let raw = action.to_raw();
                (raw.Type, raw.Delay)
            })
            .collect(),
        on_error_stop: service.get_failure_actions_on_non_crash_failures()?,
        running: matches!(service.query_status()?.current_state, ServiceState::Running | ServiceState::StartPending),
    }))
}

pub(super) fn create(name: &str, user: Option<&str>) -> anyhow::Result<()> {
    let name_w = wide(name);
    let image = wide(&command(name));
    // SAFETY: all input strings are terminated. Both SCM handles are closed
    // before returning, including when creation fails.
    unsafe {
        let manager = OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CREATE_SERVICE);
        if manager.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        let service = CreateServiceW(
            manager,
            name_w.as_ptr(),
            name_w.as_ptr(),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_DISABLED,
            SERVICE_ERROR_NORMAL,
            image.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
        );
        let error = io::Error::last_os_error();
        CloseServiceHandle(manager);
        if service.is_null() {
            return Err(error.into());
        }
        // Query/start/stop is enough for the target user. Only SYSTEM and
        // administrators can replace the SYSTEM image or its configuration.
        let user = user.map(|sid| format!("(A;;CCLCSWRPWPLOCRRC;;;{sid})")).unwrap_or_default();
        let descriptor = Descriptor::new(&format!("O:BAG:BAD:P(A;;GA;;;SY)(A;;GA;;;BA){user}"));
        let result = descriptor.and_then(|descriptor| {
            if SetServiceObjectSecurity(service, OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION, descriptor.0)
                == 0
            {
                Err(io::Error::last_os_error().into())
            } else {
                Ok(())
            }
        });
        CloseServiceHandle(service);
        result
    }
}

pub(super) fn enable(name: &str) -> anyhow::Result<()> {
    let service =
        open(name, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)?.context("guardian service disappeared")?;
    service.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::Never,
        reboot_msg: Some(OsString::new()),
        command: Some(OsString::new()),
        // SCM repeats the last action for every later failure; there is no
        // finite RestartCount as with scheduled-task failure settings.
        actions: Some(vec![ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::ZERO }]),
    })?;
    service.set_failure_actions_on_non_crash_failures(true)?;
    change_start(&service, SERVICE_AUTO_START)
}

pub(super) fn start(name: &str) -> anyhow::Result<()> {
    let service =
        open(name, ServiceAccess::START | ServiceAccess::QUERY_STATUS)?.context("guardian service is absent")?;
    if matches!(service.query_status()?.current_state, ServiceState::Running | ServiceState::StartPending) {
        return Ok(());
    }
    service.start::<&str>(&[])?;
    wait_state(name, SERVICE_NOTIFY_RUNNING | SERVICE_NOTIFY_STOPPED)?;
    if service.query_status()?.current_state != ServiceState::Running {
        bail!("guardian exited during startup");
    }
    Ok(())
}

pub(super) fn stop(name: &str) -> anyhow::Result<()> {
    let Some(service) = open(name, ServiceAccess::CHANGE_CONFIG | ServiceAccess::STOP | ServiceAccess::QUERY_STATUS)?
    else {
        return Ok(());
    };
    change_start(&service, SERVICE_DISABLED)?;
    service.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::Never,
        reboot_msg: None,
        command: None,
        actions: Some(vec![]),
    })?;
    loop {
        match service.query_status()?.current_state {
            ServiceState::Stopped => return Ok(()),
            ServiceState::StopPending => return wait_state(name, SERVICE_NOTIFY_STOPPED),
            ServiceState::StartPending => wait_state(name, SERVICE_NOTIFY_RUNNING | SERVICE_NOTIFY_STOPPED)?,
            _ => match service.stop() {
                Ok(_) => return wait_state(name, SERVICE_NOTIFY_STOPPED),
                // The process can exit between the status query and Stop.
                Err(windows_service::Error::Winapi(error))
                    if error.raw_os_error() == Some(ERROR_SERVICE_NOT_ACTIVE as i32) => {}
                Err(error) => return Err(error.into()),
            },
        }
    }
}

pub(super) fn delete(name: &str) -> anyhow::Result<()> {
    stop(name)?;
    if let Some(service) = open(name, ServiceAccess::DELETE)? {
        service.delete()?;
    }
    Ok(())
}

pub(super) fn restore(name: &str, before: Option<&Snapshot>, user: Option<&str>) -> anyhow::Result<()> {
    let Some(before) = before else { return delete(name) };
    if open(name, ServiceAccess::QUERY_CONFIG)?.is_none() {
        create(name, user)?;
    }
    let service = open(name, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)?.context("guardian disappeared")?;
    let image = wide(&command(name));
    let mut display = before.display.clone();
    display.push(0);
    // SAFETY: restore only our validated SYSTEM registration. NULL account
    // and password preserve SCM's existing account data without reading it.
    if unsafe {
        ChangeServiceConfigW(
            service.raw_handle(),
            SERVICE_NO_CHANGE,
            before.start,
            SERVICE_NO_CHANGE,
            image.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            display.as_ptr(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    let actions = before
        .actions
        .iter()
        .map(|&(kind, delay)| ServiceAction::from_raw(SC_ACTION { Type: kind, Delay: delay }))
        .collect::<Result<Vec<_>, _>>()?;
    service.update_failure_actions(ServiceFailureActions {
        reset_period: if before.reset == u32::MAX {
            ServiceFailureResetPeriod::Never
        } else {
            ServiceFailureResetPeriod::After(Duration::from_secs(u64::from(before.reset)))
        },
        reboot_msg: Some(OsString::new()),
        command: Some(OsString::new()),
        actions: Some(actions),
    })?;
    service.set_failure_actions_on_non_crash_failures(before.on_error_stop)?;
    if before.running {
        change_start(&service, SERVICE_DEMAND_START)?;
        service.start::<&str>(&[])?;
        change_start(&service, before.start)?;
    }
    Ok(())
}

fn change_start(service: &Registered, start: u32) -> anyhow::Result<()> {
    // SAFETY: NULL strings and SERVICE_NO_CHANGE preserve all other fields.
    if unsafe {
        ChangeServiceConfigW(
            service.raw_handle(),
            SERVICE_NO_CHANGE,
            start,
            SERVICE_NO_CHANGE,
            ptr::null(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

fn wait_state(name: &str, mask: u32) -> anyhow::Result<()> {
    // Close the notification's own service handle before freeing its buffer
    // on every error path; that cancels any queued callback.
    struct Pending {
        service: Registered,
        event: OwnedHandle,
        notify: Box<SERVICE_NOTIFY_2W>,
    }
    // SAFETY: the event and notification remain at stable addresses until
    // the callback completes. The waiting thread accepts the callback's APC.
    unsafe {
        let raw = CreateEventW(ptr::null(), 1, 0, ptr::null());
        if raw.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        let event = OwnedHandle::from_raw_handle(raw);
        let service = open(name, ServiceAccess::QUERY_STATUS)?.context("guardian disappeared while waiting")?;
        let notify = Box::new(SERVICE_NOTIFY_2W {
            dwVersion: 2,
            pfnNotifyCallback: Some(notified),
            pContext: event.as_raw_handle(),
            ..Default::default()
        });
        let mut pending = Pending { service, event, notify };
        let code =
            NotifyServiceStatusChangeW(pending.service.raw_handle(), mask, (&raw mut *pending.notify).cast_const());
        if code != 0 {
            return Err(io::Error::from_raw_os_error(code as i32).into());
        }
        loop {
            match WaitForSingleObjectEx(pending.event.as_raw_handle(), INFINITE, 1) {
                WAIT_OBJECT_0 => break,
                WAIT_IO_COMPLETION => continue,
                _ => return Err(io::Error::last_os_error().into()),
            }
        }
        if pending.notify.dwNotificationStatus != 0 {
            return Err(io::Error::from_raw_os_error(pending.notify.dwNotificationStatus as i32).into());
        }
    }
    Ok(())
}

unsafe extern "system" fn notified(parameter: *const std::ffi::c_void) {
    // SAFETY: SCM passes the SERVICE_NOTIFY supplied to it above.
    unsafe {
        SetEvent((*parameter.cast::<SERVICE_NOTIFY_2W>()).pContext);
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}
