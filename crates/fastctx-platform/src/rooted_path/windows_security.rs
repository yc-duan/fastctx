//! Compare a held root's protected DACL and owner with the process descriptor.
use std::{fs::File, io, os::windows::io::AsRawHandle, ptr};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::{
        Authorization::{GetSecurityInfo, SE_FILE_OBJECT},
        *,
    },
};
struct Descriptor(PSECURITY_DESCRIPTOR);
impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by GetSecurityInfo.
        unsafe { LocalFree(self.0) };
    }
}
pub(super) fn check(file: &File) -> io::Result<()> {
    let mut actual = ptr::null_mut();
    // SAFETY: query by the checked handle, not a raceable string path.
    let code = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
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
    let expected = crate::windows_user::OwnerOnly::new(true)?;
    if parts(actual.0)? != parts(expected.descriptor())? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "managed root does not have the process owner's protected ACL",
        ));
    }
    Ok(())
}
fn parts(descriptor: PSECURITY_DESCRIPTOR) -> io::Result<(Vec<u8>, Vec<u8>, bool)> {
    let (mut owner, mut acl) = (ptr::null_mut(), ptr::null_mut());
    let (mut defaulted, mut present, mut control, mut revision) = (0, 0, 0, 0);
    // SAFETY: system-produced descriptors; lengths are obtained from SID/ACL.
    unsafe {
        if GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted) == 0
            || GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) == 0
            || GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0
        {
            return Err(io::Error::last_os_error());
        }
        if owner.is_null() || present == 0 || acl.is_null() {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "missing private ACL"));
        }
        Ok((
            std::slice::from_raw_parts(owner.cast(), GetLengthSid(owner) as usize).to_vec(),
            std::slice::from_raw_parts(acl.cast(), (*acl).AclSize as usize).to_vec(),
            control & SE_DACL_PROTECTED != 0,
        ))
    }
}
