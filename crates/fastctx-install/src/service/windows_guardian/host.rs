//! The native SCM entry point. Control callbacks only record facts and wake
//! the guardian; they never perform logon, IO, or child shutdown themselves.

use std::ffi::OsString;
use std::sync::{
    Arc, OnceLock,
    atomic::Ordering,
    mpsc::{self, TrySendError},
};
use std::time::Duration;

use anyhow::{Context as _, bail};
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::ServiceControlHandlerResult;
use windows_service::{define_windows_service, service_control_handler, service_dispatcher};

use super::{
    config::Config,
    runtime::{self, Event, Requests},
    security,
};

static CONFIG: OnceLock<Config> = OnceLock::new();
define_windows_service!(ffi_main, main);

pub(super) fn dispatch(name: &str) -> anyhow::Result<()> {
    security::validate_name(name)?;
    CONFIG.set(Config::read(name)?).map_err(|_| anyhow::anyhow!("a guardian is already dispatched"))?;
    service_dispatcher::start(name, ffi_main)?;
    Ok(())
}

fn main(_arguments: Vec<OsString>) {
    if let Err(error) = serve() {
        if let Some(config) = CONFIG.get()
            && let Ok(root) = config.root()
            && let Ok(file) = fastctx_platform::rolling_log::RollingLog::open(&root.join("guardian.log"))
        {
            let _ = file.write(format!("guardian failed: {error:#}\n").as_bytes());
        }
        // Exiting without SERVICE_STOPPED is an SCM failure even if the
        // failure happened before a status handle could be registered.
        std::process::exit(1);
    }
}

fn serve() -> anyhow::Result<()> {
    let config = CONFIG.get().context("guardian configuration was not provided")?.clone();
    let requests = Arc::new(Requests::default());
    // Two nodes can each have a lifecycle message, a protocol failure and
    // an exit notification pending; one coalesced SCM wake makes seven.
    let (sender, events) = mpsc::sync_channel(2 * 3 + 1);
    let pending = requests.clone();
    let wake = sender.clone();
    let status = service_control_handler::register(&config.name, move |control| {
        match control {
            ServiceControl::Interrogate => return ServiceControlHandlerResult::NoError,
            ServiceControl::Stop => pending.stop.store(true, Ordering::SeqCst),
            ServiceControl::Shutdown | ServiceControl::Preshutdown => pending.shutdown.store(true, Ordering::SeqCst),
            ServiceControl::UserEvent(code) if code.to_raw() == super::adopt::ADOPT => {
                pending.adopt.store(true, Ordering::SeqCst)
            }
            ServiceControl::UserEvent(code) if code.to_raw() == super::adopt::PLANNED => {
                pending.planned.store(true, Ordering::SeqCst);
                return ServiceControlHandlerResult::NoError;
            }
            ServiceControl::SessionChange(_)
            | ServiceControl::PowerEvent(_)
            | ServiceControl::TimeChange
            | ServiceControl::UserEvent(_) => pending.refresh.store(true, Ordering::SeqCst),
            _ => return ServiceControlHandlerResult::NotImplemented,
        }
        // When the queue is full the reader is already awake, and checks
        // these atomic requests after each event. There is no lost request.
        if !pending.wake_queued.swap(true, Ordering::SeqCst) {
            match wake.try_send(Event::Wake) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                    pending.wake_queued.store(false, Ordering::SeqCst)
                }
            }
        }
        ServiceControlHandlerResult::NoError
    })?;
    let report = |state, code, controls| {
        status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: controls,
            exit_code: ServiceExitCode::Win32(code),
            checkpoint: 0,
            wait_hint: Duration::ZERO,
            process_id: None,
        })
    };
    report(ServiceState::StartPending, 0, ServiceControlAccept::empty())?;
    let ready = || {
        super::renewal::record(&config)?;
        report(
            ServiceState::Running,
            0,
            ServiceControlAccept::STOP
                | ServiceControlAccept::SHUTDOWN
                | ServiceControlAccept::SESSION_CHANGE
                | ServiceControlAccept::POWER_EVENT,
        )
        .map_err(Into::into)
    };
    let result = runtime::run(config.clone(), requests.clone(), events, sender, ready);
    // A stop while still installed is recoverable, too: it ends as a failure,
    // which the service manager answers by starting the guardian again, as
    // it does a hub's guardian that took a newer program by itself.
    // Uninstall disables failure actions and auto-start before requesting
    // this same stop. A stop its user said was planned, to restart it on a
    // new image, ends well.
    let planned = requests.planned.load(Ordering::SeqCst) && requests.stop.load(Ordering::SeqCst);
    let code = if requests.shutdown.load(Ordering::SeqCst) || planned { 0 } else { 1 };
    report(ServiceState::Stopped, code, ServiceControlAccept::empty())?;
    if let Err(error) = result {
        bail!("{error:#}");
    }
    Ok(())
}
