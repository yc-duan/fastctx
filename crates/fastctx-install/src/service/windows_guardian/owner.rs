//! Preserve the caller's identity and privileges while making Administrators
//! the owner of staged files. An inherited read-only ACL is insufficient if
//! the target user owns a file and can therefore replace its DACL.

use super::security::Descriptor;
use anyhow::Context as _;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::{io, mem::size_of, ptr};
use windows_sys::Win32::Foundation::ERROR_NO_TOKEN;
use windows_sys::Win32::Security::{
    DuplicateTokenEx, GetSecurityDescriptorOwner, SecurityImpersonation, SetTokenInformation, TOKEN_ADJUST_DEFAULT,
    TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_OWNER, TOKEN_QUERY, TokenImpersonation, TokenOwner,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken, SetThreadToken,
};

pub(super) struct AdministratorOwner(Option<OwnedHandle>);

impl AdministratorOwner {
    pub fn enter() -> anyhow::Result<Self> {
        let mut raw = ptr::null_mut();
        let access = TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_IMPERSONATE;
        let previous = if unsafe { OpenThreadToken(GetCurrentThread(), access, 1, &mut raw) } != 0 {
            Some(unsafe { OwnedHandle::from_raw_handle(raw) })
        } else if io::Error::last_os_error().raw_os_error() == Some(ERROR_NO_TOKEN as i32) {
            None
        } else {
            return Err(io::Error::last_os_error().into());
        };
        let process = if previous.is_none() {
            if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY | TOKEN_DUPLICATE, &mut raw) } == 0 {
                return Err(io::Error::last_os_error().into());
            }
            Some(unsafe { OwnedHandle::from_raw_handle(raw) })
        } else {
            None
        };
        let source = previous.as_ref().or(process.as_ref()).context("caller token is absent")?;
        if unsafe {
            DuplicateTokenEx(
                source.as_raw_handle(),
                TOKEN_QUERY | TOKEN_IMPERSONATE | TOKEN_ADJUST_DEFAULT,
                ptr::null(),
                SecurityImpersonation,
                TokenImpersonation,
                &mut raw,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        let token = unsafe { OwnedHandle::from_raw_handle(raw) };
        let descriptor = Descriptor::new("O:BA")?;
        let mut owner = ptr::null_mut();
        let mut defaulted = 0;
        if unsafe { GetSecurityDescriptorOwner(descriptor.0, &mut owner, &mut defaulted) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        let owner = TOKEN_OWNER { Owner: owner };
        if unsafe {
            SetTokenInformation(
                token.as_raw_handle(),
                TokenOwner,
                (&raw const owner).cast(),
                size_of::<TOKEN_OWNER>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error())
                .context("service installation requires an administrator owner token");
        }
        if unsafe { SetThreadToken(ptr::null(), token.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self(previous))
    }
}

impl Drop for AdministratorOwner {
    fn drop(&mut self) {
        let previous = self.0.as_ref().map(|token| token.as_raw_handle()).unwrap_or(ptr::null_mut());
        if unsafe { SetThreadToken(ptr::null(), previous) } == 0 {
            std::process::abort();
        }
    }
}
