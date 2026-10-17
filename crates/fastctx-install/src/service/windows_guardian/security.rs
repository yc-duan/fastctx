//! The SYSTEM image and its configuration must never become writable through
//! a normal user token, including the target account's filtered admin token.

use std::io;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::ptr;

use anyhow::{Context as _, bail};
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;

pub(super) struct Descriptor(pub *mut std::ffi::c_void);

impl Descriptor {
    pub fn new(sddl: &str) -> anyhow::Result<Self> {
        let text: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
        let mut descriptor = ptr::null_mut();
        // SAFETY: the source is terminated and Windows owns the allocation.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(text.as_ptr(), 1, &mut descriptor, ptr::null_mut())
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self(descriptor))
    }
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

pub(super) fn directory(name: &str) -> anyhow::Result<PathBuf> {
    validate_name(name)?;
    Ok(services()?.join(name))
}

/// Where every guardian's directory is.
pub(super) fn services() -> anyhow::Result<PathBuf> {
    Ok(program_data()?.join("fastctx-services"))
}

pub(super) fn prepare(name: &str, user: Option<&str>) -> anyhow::Result<PathBuf> {
    let directory = directory(name)?;
    // Users can traverse the shared parent, but cannot list, create, delete
    // or replace its children. Scoped directories grant only their user read.
    secure_directory(
        directory.parent().context("guardian directory has no parent")?,
        "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;;FX;;;BU)",
    )?;
    let user = user.map(|sid| format!("(A;OICI;FRFX;;;{sid})")).unwrap_or_default();
    secure_directory(&directory, &format!("O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA){user}"))?;
    Ok(directory)
}

pub(super) fn validate_name(name: &str) -> anyhow::Result<()> {
    if !name.starts_with("fastctx-service-")
        || !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        bail!("invalid guardian service name");
    }
    Ok(())
}

fn secure_directory(path: &Path, sddl: &str) -> anyhow::Result<()> {
    Ok(fastctx_platform::protected_directory::ensure(path, sddl)?)
}

fn program_data() -> anyhow::Result<PathBuf> {
    Ok(fastctx_platform::protected_directory::program_data()?)
}
