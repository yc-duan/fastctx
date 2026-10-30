//! Which user this process runs as and who owns an object, named by SID,
//! and access lists that admit only this user.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt as _;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
    SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_OWNER, TOKEN_QUERY, TOKEN_USER,
    TokenOwner, TokenUser,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// The current process's user, as text such as `S-1-5-21-…`.
pub fn current_user_sid() -> io::Result<String> {
    with_current_token(token_user_sid)
}

/// Runs `query` with this process's own token, opened for queries.
pub fn with_current_token<R>(query: impl FnOnce(HANDLE) -> io::Result<R>) -> io::Result<R> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: opening this process's own token; closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let result = query(token);
    // SAFETY: the token handle is owned here.
    unsafe { CloseHandle(token) };
    result
}

pub(crate) fn token_user_sid(token: HANDLE) -> io::Result<String> {
    let mut needed = 0u32;
    // SAFETY: a size query with no buffer.
    unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed) };
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: the buffer is at least `needed` bytes and aligned.
    if unsafe { GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), needed, &mut needed) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: GetTokenInformation filled a TOKEN_USER.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    sid_text(user.User.Sid)
}

/// The owner this process gives the objects it creates: its user, or the
/// administrators' group for an elevated administrator.
pub(crate) fn current_owner_sid() -> io::Result<String> {
    with_current_token(|token| {
        let mut needed = 0u32;
        // SAFETY: a size query with no buffer.
        unsafe { GetTokenInformation(token, TokenOwner, ptr::null_mut(), 0, &mut needed) };
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
        // SAFETY: the buffer is at least `needed` bytes and aligned.
        if unsafe { GetTokenInformation(token, TokenOwner, buffer.as_mut_ptr().cast(), needed, &mut needed) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: GetTokenInformation filled a TOKEN_OWNER.
        let owner = unsafe { &*buffer.as_ptr().cast::<TOKEN_OWNER>() };
        sid_text(owner.Owner)
    })
}

/// The owner of a kernel object, such as a pipe, given a handle with read
/// access to its security.
pub fn object_owner_sid(handle: HANDLE) -> io::Result<String> {
    let mut owner: PSID = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: out-parameters only; the descriptor holding the owner is
    // freed below.
    let code = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    let sid = sid_text(owner);
    // SAFETY: allocated by GetSecurityInfo.
    unsafe { LocalFree(descriptor) };
    sid
}

/// The name of the account `sid` names, as its own sessions see it in
/// `USERNAME`.
pub fn account_name(sid: &str) -> io::Result<String> {
    use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
    use windows_sys::Win32::Security::LookupAccountSidW;
    let wide: Vec<u16> = OsStr::new(sid).encode_wide().chain([0]).collect();
    let mut raw: PSID = ptr::null_mut();
    // SAFETY: a NUL-terminated string; the SID made from it is freed below.
    if unsafe { ConvertStringSidToSidW(wide.as_ptr(), &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let named = (|| {
        let (mut name_length, mut domain_length, mut kind) = (0u32, 0u32, 0);
        // SAFETY: measures the two buffers; the SID stays live.
        unsafe {
            LookupAccountSidW(
                ptr::null(),
                raw,
                ptr::null_mut(),
                &mut name_length,
                ptr::null_mut(),
                &mut domain_length,
                &mut kind,
            )
        };
        let mut name = vec![0u16; name_length as usize];
        let mut domain = vec![0u16; domain_length as usize];
        // SAFETY: the buffers have the measured lengths.
        if unsafe {
            LookupAccountSidW(
                ptr::null(),
                raw,
                name.as_mut_ptr(),
                &mut name_length,
                domain.as_mut_ptr(),
                &mut domain_length,
                &mut kind,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        name.truncate(name_length as usize);
        Ok(String::from_utf16_lossy(&name))
    })();
    // SAFETY: allocated by ConvertStringSidToSidW.
    unsafe { LocalFree(raw) };
    named
}

fn sid_text(sid: PSID) -> io::Result<String> {
    let mut text: *mut u16 = ptr::null_mut();
    // SAFETY: a valid SID; the string is freed below.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: ConvertSidToStringSidW returns a NUL-terminated string.
    let length = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
    // SAFETY: `length` units precede the NUL.
    let sid = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    // SAFETY: allocated by ConvertSidToStringSidW.
    unsafe { LocalFree(text.cast()) };
    Ok(sid)
}

/// A security descriptor owned by the current user whose protected access
/// list grants full control to that user and to SYSTEM, and nothing to
/// anyone else. The user is named explicitly rather than left to the
/// default owner, because an elevated process would make Administrators the
/// owner, after which the same user without elevation would be locked out.
/// Nobody else can make an object owned by this user, so the owner also
/// proves who made it.
pub struct OwnerOnly(PSECURITY_DESCRIPTOR);

// SAFETY: the descriptor is immutable after creation and freed only on drop.
unsafe impl Send for OwnerOnly {}
// SAFETY: as above; shared use only reads it.
unsafe impl Sync for OwnerOnly {}

impl OwnerOnly {
    /// `inherited` makes files and directories created inside a directory
    /// carrying this list inherit it.
    pub fn new(inherited: bool) -> io::Result<Self> {
        Self::with_system(inherited, true)
    }

    /// The exact user-only descriptor required by Codex's socket clients.
    pub fn user_only() -> io::Result<Self> {
        Self::with_system(true, false)
    }

    pub(crate) fn administrator_handoff() -> io::Result<Self> {
        Self::with_groups(true, true, true)
    }

    fn with_system(inherited: bool, system: bool) -> io::Result<Self> {
        Self::with_groups(inherited, system, false)
    }

    fn with_groups(inherited: bool, system: bool, administrators: bool) -> io::Result<Self> {
        let sid = current_user_sid()?;
        let flags = if inherited { "OICI" } else { "" };
        let system = if system { format!("(A;{flags};FA;;;SY)") } else { String::new() };
        let administrators = if administrators { format!("(A;{flags};FA;;;BA)") } else { String::new() };
        let sddl: Vec<u16> = OsStr::new(&format!("O:{sid}D:P(A;{flags};FA;;;{sid}){system}{administrators}"))
            .encode_wide()
            .chain([0])
            .collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: a NUL-terminated SDDL string; the descriptor is freed on drop.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
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

    pub fn descriptor(&self) -> PSECURITY_DESCRIPTOR {
        self.0
    }
}

impl Drop for OwnerOnly {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(self.0) };
    }
}
