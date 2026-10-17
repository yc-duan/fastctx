//! Directories that SYSTEM keeps for Fastctx under `%ProgramData%`, where
//! every user may create folders: one is made with its owner and protected
//! access list from its first visible instant, and one found already there
//! is used only if it is a real directory carrying exactly those, since a
//! folder someone else made first could lead SYSTEM to run what they put in.

use std::io;
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::windows::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetLengthSid, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
    GetSecurityDescriptorOwner, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED,
    SECURITY_ATTRIBUTES,
};
use windows_sys::Win32::Storage::FileSystem::{CreateDirectoryW, FILE_ATTRIBUTE_REPARSE_POINT};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramData, SHGetKnownFolderPath};

/// The machine-wide application data folder, normally `C:\ProgramData`.
pub fn program_data() -> io::Result<PathBuf> {
    let mut value = ptr::null_mut();
    // SAFETY: the known folder's allocation is copied, then released.
    unsafe {
        let code = SHGetKnownFolderPath(&FOLDERID_ProgramData, 0, ptr::null_mut(), &mut value);
        if code < 0 {
            return Err(io::Error::other(format!("finding ProgramData failed: HRESULT {code:#x}")));
        }
        let length = (0..).take_while(|&i| *value.add(i) != 0).count();
        let path = std::ffi::OsString::from_wide(std::slice::from_raw_parts(value, length));
        CoTaskMemFree(value.cast());
        Ok(path.into())
    }
}

/// Makes `path` with the owner and access list `sddl` states, or confirms
/// that the directory there has exactly those and is no link.
pub fn ensure(path: &Path, sddl: &str) -> io::Result<()> {
    let descriptor = Descriptor::new(sddl)?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    // SAFETY: the path and attributes outlive the call.
    if unsafe { CreateDirectoryW(wide.as_ptr(), &attributes) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32) {
            return Err(error);
        }
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other(format!("{} is not a plain directory", path.display())));
    }
    let mut actual = ptr::null_mut();
    // SAFETY: Windows returns one self-contained allocation, freed by the wrapper.
    let code = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
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
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    let actual = Descriptor(actual);
    if parts(actual.0)? != parts(descriptor.0)? {
        return Err(io::Error::other(format!(
            "{} was not made by Fastctx: its owner or access list is another; it is left as it is",
            path.display()
        )));
    }
    Ok(())
}

struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    fn new(sddl: &str) -> io::Result<Self> {
        let text: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
        let mut descriptor = ptr::null_mut();
        // SAFETY: the source is terminated and Windows owns the allocation.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(descriptor))
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by Windows for this wrapper alone.
        unsafe { LocalFree(self.0) };
    }
}

/// The owner, the access list and whether it is protected from inheriting.
fn parts(descriptor: PSECURITY_DESCRIPTOR) -> io::Result<(Vec<u8>, Vec<u8>, bool)> {
    let (mut owner, mut acl) = (ptr::null_mut(), ptr::null_mut());
    let (mut defaulted, mut present, mut control, mut revision) = (0, 0, 0, 0);
    // SAFETY: the descriptor and all out-parameters are live; lengths are
    // reported by the SID and ACL structures, not guessed from their content.
    unsafe {
        if GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted) == 0
            || GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) == 0
            || GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0
        {
            return Err(io::Error::last_os_error());
        }
        if owner.is_null() || present == 0 || acl.is_null() {
            return Err(io::Error::other("the security descriptor lacks an owner or an access list"));
        }
        Ok((
            std::slice::from_raw_parts(owner.cast(), GetLengthSid(owner) as usize).to_vec(),
            std::slice::from_raw_parts(acl.cast(), (*acl).AclSize as usize).to_vec(),
            control & SE_DACL_PROTECTED != 0,
        ))
    }
}
