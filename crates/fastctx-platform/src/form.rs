//! What kind of device this machine is, as an interface draws it beside a
//! conversation (`world-gui.md`, 用词): a desktop, a laptop or a server.
//!
//! A machine with a system battery is a laptop. Without one, a Windows or
//! macOS machine is a desktop and a Linux one a server: the Linux machines
//! that join a World are almost always servers, and a Linux desktop drawn as
//! a server still says its name and system when pointed at.

use fastctx_types::world::Form;

/// This machine's kind, read once from the system.
pub fn form() -> Form {
    if battery() {
        Form::Laptop
    } else if cfg!(target_os = "linux") {
        Form::Server
    } else {
        Form::Desktop
    }
}

#[cfg(windows)]
fn battery() -> bool {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    // SAFETY: the structure is plain data the call fills in.
    let mut status: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: `status` is a valid, writable SYSTEM_POWER_STATUS.
    if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
        return false;
    }
    // 128: "No system battery"; 255: the status cannot be read.
    status.BatteryFlag != 128 && status.BatteryFlag != 255
}

#[cfg(target_os = "linux")]
fn battery() -> bool {
    let Ok(supplies) = std::fs::read_dir("/sys/class/power_supply") else { return false };
    supplies.flatten().any(|supply| {
        std::fs::read_to_string(supply.path().join("type")).is_ok_and(|kind| kind.trim() == "Battery")
            && std::fs::read_to_string(supply.path().join("scope")).map_or(true, |scope| scope.trim() != "Device")
    })
}

#[cfg(not(any(windows, target_os = "linux")))]
fn battery() -> bool {
    std::process::Command::new("pmset")
        .args(["-g", "batt"])
        .output()
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains("InternalBattery"))
}
