//! The SYSTEM image and its configuration must never become writable through
//! a normal user token, including the target account's filtered admin token.

use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::ptr;

use anyhow::{Context as _, bail};
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
    OWNER_SECURITY_INFORMATION, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES,
};
use windows_sys::Win32::Storage::FileSystem::{CreateDirectoryW, FILE_ATTRIBUTE_REPARSE_POINT};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramData, SHGetKnownFolderPath};

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
    Ok(program_data()?.join("fastctx-services").join(name))
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
    let descriptor = Descriptor::new(sddl)?;
    let path_w: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: the directory is protected from its first visible instant.
    if unsafe { CreateDirectoryW(path_w.as_ptr(), &descriptor.attributes()) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32) {
            return Err(error.into());
        }
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        bail!("{} is not a regular guardian directory", path.display());
    }
    let mut actual = ptr::null_mut();
    // SAFETY: Windows returns one self-contained allocation, freed below.
    let code = unsafe {
        GetNamedSecurityInfoW(
            path_w.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut actual,
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32).into());
    }
    let actual = Descriptor(actual);
    if security_parts(actual.0)? != security_parts(descriptor.0)? {
        bail!("{} does not have the guardian's protected owner and access list", path.display());
    }
    Ok(())
}

fn security_parts(descriptor: *mut std::ffi::c_void) -> anyhow::Result<(Vec<u8>, Vec<u8>, bool)> {
    use windows_sys::Win32::Security::GetLengthSid;
    let (mut owner, mut acl) = (ptr::null_mut(), ptr::null_mut());
    let (mut defaulted, mut present, mut control, mut revision) = (0, 0, 0, 0);
    // SAFETY: the descriptor and all out-parameters are live; lengths are
    // reported by the SID/ACL structures, not guessed from their content.
    unsafe {
        if GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted) == 0
            || GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) == 0
            || GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        if owner.is_null() || present == 0 || acl.is_null() {
            bail!("guardian security descriptor lacks owner or DACL");
        }
        Ok((
            std::slice::from_raw_parts(owner.cast(), GetLengthSid(owner) as usize).to_vec(),
            std::slice::from_raw_parts(acl.cast(), (*acl).AclSize as usize).to_vec(),
            control & SE_DACL_PROTECTED != 0,
        ))
    }
}

fn program_data() -> anyhow::Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt as _;
    let mut value = ptr::null_mut();
    // SAFETY: this known-folder allocation is copied then released.
    unsafe {
        let code = SHGetKnownFolderPath(&FOLDERID_ProgramData, 0, ptr::null_mut(), &mut value);
        if code < 0 {
            bail!("finding ProgramData failed: HRESULT {code:#x}");
        }
        let length = (0..).take_while(|&i| *value.add(i) != 0).count();
        let path = std::ffi::OsString::from_wide(std::slice::from_raw_parts(value, length));
        CoTaskMemFree(value.cast());
        Ok(path.into())
    }
}
