use std::collections::BTreeMap;
use std::ffi::c_void;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::Notify;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::NetworkManagement::IpHelper::{
    CancelMibChangeNotify2, MIB_IPFORWARD_ROW2, MIB_IPINTERFACE_ROW, MIB_NOTIFICATION_TYPE, MIB_UNICASTIPADDRESS_ROW,
    NotifyIpInterfaceChange, NotifyRouteChange2, NotifyUnicastIpAddressChange,
};
use windows_sys::Win32::Networking::WinSock::AF_UNSPEC;
use windows_sys::Win32::System::Power::{
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, PowerRegisterSuspendResumeNotification,
    PowerUnregisterSuspendResumeNotification,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC};

pub(crate) struct Changes {
    signal: Arc<Notify>,
    id: usize,
    interfaces: HANDLE,
    addresses: HANDLE,
    routes: HANDLE,
    power: HANDLE,
}

// The registration handles are opaque, not thread-affine. Callback context
// is an integer identity, never a pointer dereferenced by a callback.
unsafe impl Send for Changes {}

static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
static SIGNALS: Mutex<BTreeMap<usize, Weak<Notify>>> = Mutex::new(BTreeMap::new());

fn register(signal: &Arc<Notify>) -> usize {
    // Identities must never be reused: a late callback must not notify a
    // new registration after the old one has been removed.
    let id = NEXT_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .expect("network notification identities exhausted");
    SIGNALS.lock().expect("never poisoned").insert(id, Arc::downgrade(signal));
    id
}

impl Changes {
    pub fn new(_: Arc<dyn super::super::Node>) -> io::Result<Self> {
        let signal = Arc::new(Notify::new());
        let mut changes = Self {
            id: register(&signal),
            signal,
            interfaces: std::ptr::null_mut(),
            addresses: std::ptr::null_mut(),
            routes: std::ptr::null_mut(),
            power: std::ptr::null_mut(),
        };
        let context = changes.id as *mut c_void;
        // SAFETY: the OS passes this opaque identity back unchanged. The
        // callbacks look it up rather than dereferencing it. Drop cancels
        // each successful registration and removes the lookup entry.
        unsafe {
            check(NotifyIpInterfaceChange(AF_UNSPEC, Some(interface), context, false, &mut changes.interfaces))?;
            check(NotifyUnicastIpAddressChange(AF_UNSPEC, Some(address), context, false, &mut changes.addresses))?;
            check(NotifyRouteChange2(AF_UNSPEC, Some(route), context, false, &mut changes.routes))?;
            let subscription = DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS { Callback: Some(power), Context: context };
            check(PowerRegisterSuspendResumeNotification(
                DEVICE_NOTIFY_CALLBACK,
                (&raw const subscription).cast_mut().cast(),
                &mut changes.power,
            ))?;
        }
        Ok(changes)
    }

    pub async fn changed(&mut self) -> io::Result<&'static str> {
        self.signal.notified().await;
        Ok("network interfaces, addresses or routes changed, or the machine resumed")
    }
}

impl Drop for Changes {
    fn drop(&mut self) {
        // Cancellation must happen outside callbacks: CancelMibChangeNotify2
        // waits for outstanding callbacks. The lookup lock is not held
        // during cancellation. PowerUnregister's public contract does not
        // promise a callback barrier, so late power callbacks use the same
        // identity lookup and become no-ops after removal.
        unsafe {
            for handle in [self.interfaces, self.addresses, self.routes] {
                if !handle.is_null() {
                    cancelled(CancelMibChangeNotify2(handle));
                }
            }
            if !self.power.is_null() {
                cancelled(PowerUnregisterSuspendResumeNotification(self.power as isize));
            }
        }
        SIGNALS.lock().expect("never poisoned").remove(&self.id);
    }
}

fn cancelled(code: u32) {
    // A failed cancellation leaves an OS registration alive. Surface the
    // failure and release all process registrations by terminating.
    if code != 0 {
        eprintln!("cancelling network/resume notifications failed: Windows error {code}");
        std::process::abort();
    }
}

fn check(code: u32) -> io::Result<()> {
    if code == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(code as _)) }
}

unsafe fn notify(context: *const c_void) {
    let signal = SIGNALS.lock().expect("never poisoned").get(&(context as usize)).and_then(Weak::upgrade);
    if let Some(signal) = signal {
        signal.notify_one();
    }
}

unsafe extern "system" fn interface(context: *const c_void, _: *const MIB_IPINTERFACE_ROW, _: MIB_NOTIFICATION_TYPE) {
    unsafe {
        notify(context);
    }
}
unsafe extern "system" fn address(
    context: *const c_void,
    _: *const MIB_UNICASTIPADDRESS_ROW,
    _: MIB_NOTIFICATION_TYPE,
) {
    unsafe {
        notify(context);
    }
}
unsafe extern "system" fn route(context: *const c_void, _: *const MIB_IPFORWARD_ROW2, _: MIB_NOTIFICATION_TYPE) {
    unsafe {
        notify(context);
    }
}
unsafe extern "system" fn power(context: *const c_void, event: u32, _: *const c_void) -> u32 {
    // PBT_APMRESUMEAUTOMATIC is delivered on every resume; the interactive
    // PBT_APMRESUMESUSPEND can follow much later and is not another wake.
    if event == PBT_APMRESUMEAUTOMATIC {
        unsafe {
            notify(context);
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt as _;

    #[test]
    fn late_callbacks_cannot_access_retired_or_subsequent_registrations() {
        let first = Arc::new(Notify::new());
        let old = register(&first);
        unsafe {
            power(old as _, PBT_APMRESUMEAUTOMATIC, std::ptr::null());
        }
        assert!(first.notified().now_or_never().is_some());
        SIGNALS.lock().unwrap().remove(&old);
        drop(first);

        let next = Arc::new(Notify::new());
        let current = register(&next);
        assert_ne!(old, current);
        unsafe {
            power(old as _, PBT_APMRESUMEAUTOMATIC, std::ptr::null());
        }
        assert!(next.notified().now_or_never().is_none());
        unsafe {
            power(current as _, PBT_APMRESUMEAUTOMATIC, std::ptr::null());
        }
        assert!(next.notified().now_or_never().is_some());
        SIGNALS.lock().unwrap().remove(&current);
    }
}
