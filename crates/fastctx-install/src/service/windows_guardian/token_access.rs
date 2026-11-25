//! What the processes a node's token starts let other programs of the same
//! account do with them by default.

use std::io;
use std::mem::{size_of, zeroed};
use std::ptr;

use windows_sys::Win32::Foundation::{HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetTokenInformation, InitializeSecurityDescriptor,
    SECURITY_DESCRIPTOR, SetSecurityDescriptorDacl, SetTokenInformation, TOKEN_DEFAULT_DACL, TokenDefaultDacl,
};

/// Lets the account `sid` into what is made with `token` by default, as the
/// account's own desktop logon lets it in. A token for a logon nobody sits at,
/// as a node starts with before anyone logs on, lets only Administrators,
/// SYSTEM and that logon in; the account's desktop programs under User
/// Account Control are none of those, and could not look at the node they
/// talk to (`fastctx_platform::ipc`).
pub(super) fn admit_account(token: HANDLE, sid: &str) -> io::Result<()> {
    let current = default_dacl(token)?;
    let ace = format!("(A;;GA;;;{sid})");
    if current.contains(&ace) {
        return Ok(());
    }
    let sddl = match current.find('(') {
        Some(at) => format!("{}{ace}{}", &current[..at], &current[at..]),
        // No default access at all, or none that can be added to.
        None => format!("D:{ace}(A;;GA;;;SY)(A;;GA;;;BA)"),
    };
    set_default_dacl(token, &sddl)
}

/// The token's default DACL, in SDDL.
fn default_dacl(token: HANDLE) -> io::Result<String> {
    let mut length = 0;
    // SAFETY: asks only for the size the information needs.
    unsafe { GetTokenInformation(token, TokenDefaultDacl, ptr::null_mut(), 0, &mut length) };
    if length == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut buffer = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
    // SAFETY: the buffer holds `length` bytes, aligned for the structure.
    if unsafe { GetTokenInformation(token, TokenDefaultDacl, buffer.as_mut_ptr().cast(), length, &mut length) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: Windows wrote a TOKEN_DEFAULT_DACL at the start of the buffer,
    // its ACL inside the same buffer.
    let dacl = unsafe { (*buffer.as_ptr().cast::<TOKEN_DEFAULT_DACL>()).DefaultDacl };
    // SAFETY: an absolute descriptor on the stack that only refers to the
    // ACL above, both alive for the conversion.
    unsafe {
        let mut descriptor: SECURITY_DESCRIPTOR = zeroed();
        let pointer = (&raw mut descriptor).cast();
        if InitializeSecurityDescriptor(pointer, 1) == 0 || SetSecurityDescriptorDacl(pointer, 1, dacl, 0) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut text = ptr::null_mut();
        if ConvertSecurityDescriptorToStringSecurityDescriptorW(
            pointer,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut text,
            ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut units = 0;
        while *text.add(units) != 0 {
            units += 1;
        }
        let sddl = String::from_utf16_lossy(std::slice::from_raw_parts(text, units));
        LocalFree(text.cast());
        Ok(sddl)
    }
}

/// Makes `sddl`'s DACL the token's default one.
fn set_default_dacl(token: HANDLE, sddl: &str) -> io::Result<()> {
    let text: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
    let mut descriptor = ptr::null_mut();
    // SAFETY: the source is terminated; Windows allocates the descriptor,
    // freed below.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(text.as_ptr(), SDDL_REVISION_1, &mut descriptor, ptr::null_mut())
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the DACL lives inside the descriptor, which outlives the call
    // that copies it into the token.
    let result = unsafe {
        let (mut present, mut defaulted, mut dacl) = (0, 0, ptr::null_mut::<ACL>());
        if GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) == 0 {
            Err(io::Error::last_os_error())
        } else {
            let value = TOKEN_DEFAULT_DACL { DefaultDacl: dacl };
            if SetTokenInformation(
                token,
                TokenDefaultDacl,
                (&raw const value).cast(),
                size_of::<TOKEN_DEFAULT_DACL>() as u32,
            ) == 0
            {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
    };
    // SAFETY: allocated by the conversion above.
    unsafe { LocalFree(descriptor) };
    result
}
