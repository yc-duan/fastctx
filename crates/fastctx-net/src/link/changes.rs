//! Notifications invalidate a connection attempt as well as an established
//! connection. One pending notification is enough: every dial reads the
//! interfaces and routes again.

#[cfg(target_os = "linux")]
#[path = "changes/linux.rs"]
mod platform;
#[cfg(windows)]
#[path = "changes/windows.rs"]
mod platform;

#[cfg(any(target_os = "linux", windows))]
pub(super) use platform::Changes;

#[cfg(not(any(target_os = "linux", windows)))]
pub(super) struct Changes;

#[cfg(not(any(target_os = "linux", windows)))]
impl Changes {
    pub fn new(_: std::sync::Arc<dyn super::Node>) -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "network change notifications are not implemented on this platform",
        ))
    }

    pub async fn changed(&mut self) -> std::io::Result<&'static str> {
        unreachable!()
    }
}
