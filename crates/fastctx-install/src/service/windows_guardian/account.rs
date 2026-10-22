//! Windows creates the user's logon and environment. No password, credential
//! file, cached secret, or authentication-package profile data is read here.

use std::ffi::OsString;
use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStringExt as _;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::ptr;

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_NO_TOKEN, ERROR_SUCCESS, HANDLE, LUID, LocalFree, SetLastError,
};
use windows_sys::Win32::Security::Authentication::Identity::{
    LSA_STRING, LSA_UNICODE_STRING, LsaDeregisterLogonProcess, LsaFreeReturnBuffer, LsaLogonUser,
    LsaLookupAuthenticationPackage, LsaNtStatusToWinError, LsaRegisterLogonProcess, MSV1_0_S4U_LOGON, MsV1_0S4ULogon,
    SECURITY_LOGON_TYPE,
};
use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSidToSidW};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, AllocateLocallyUniqueId, DuplicateTokenEx, GetTokenInformation, LUID_AND_ATTRIBUTES,
    LookupAccountSidW, LookupPrivilegeValueW, QUOTA_LIMITS, SE_PRIVILEGE_ENABLED, SecurityImpersonation,
    SetTokenInformation, SidTypeUser, TOKEN_ADJUST_PRIVILEGES, TOKEN_ALL_ACCESS, TOKEN_LINKED_TOKEN, TOKEN_PRIVILEGES,
    TOKEN_QUERY, TOKEN_SOURCE, TOKEN_STATISTICS, TOKEN_USER, TokenElevationType, TokenElevationTypeLimited,
    TokenLinkedToken, TokenPrimary, TokenSessionId, TokenStatistics, TokenUser,
};
use windows_sys::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows_sys::Win32::System::RemoteDesktop::{
    WTS_CURRENT_SERVER_HANDLE, WTSDown, WTSEnumerateSessionsW, WTSFreeMemory, WTSInit, WTSListen, WTSQueryUserToken,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::UI::Shell::{GetUserProfileDirectoryW, LoadUserProfileW, PROFILEINFOW, UnloadUserProfile};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Target {
    pub sid: String,
    pub user: Vec<u16>,
    pub domain: Vec<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Identity {
    S4u,
    Interactive(u64),
    System,
}

pub(super) struct Login {
    pub token: OwnedHandle,
    pub identity: Identity,
    pub home: OsString,
    profile: HANDLE,
}

pub(super) struct Environment(*mut std::ffi::c_void);

impl Environment {
    pub fn new(token: Option<HANDLE>) -> anyhow::Result<Self> {
        let mut block = ptr::null_mut();
        // SAFETY: Windows allocates the opaque block, released in Drop.
        if unsafe { CreateEnvironmentBlock(&mut block, token.unwrap_or(ptr::null_mut()), 0) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self(block))
    }
    pub fn as_ptr(&self) -> *mut std::ffi::c_void {
        self.0
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        // SAFETY: this block was allocated by CreateEnvironmentBlock.
        unsafe {
            DestroyEnvironmentBlock(self.0);
        }
    }
}

impl Drop for Login {
    fn drop(&mut self) {
        // SAFETY: the profile was loaded with this token and remains live.
        if unsafe { UnloadUserProfile(self.token.as_raw_handle(), self.profile) } == 0 {
            eprintln!("fastctx guardian: unloading the user profile: {}", io::Error::last_os_error());
        }
    }
}

pub(super) fn target(requested: Option<&str>) -> anyhow::Result<Target> {
    if let Some(requested) = requested {
        let wide: Vec<u16> = requested.encode_utf16().chain([0]).collect();
        if requested.contains('\0') {
            bail!("a SID cannot contain NUL");
        }
        let mut raw = ptr::null_mut();
        if unsafe { ConvertStringSidToSidW(wide.as_ptr(), &mut raw) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        let result = target_from_sid(raw);
        unsafe {
            LocalFree(raw);
        }
        return result;
    }
    let token = current_token(TOKEN_QUERY)?;
    let buffer = token_info(token.as_raw_handle(), TokenUser)?;
    // SAFETY: token_info returns aligned storage of the queried type.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    target_from_sid(user.User.Sid)
}

fn target_from_sid(raw: *mut std::ffi::c_void) -> anyhow::Result<Target> {
    let sid = sid_text(raw)?;
    let (mut name_len, mut domain_len, mut kind) = (0, 0, 0);
    // SAFETY: the first call measures the two buffers; the SID stays live.
    unsafe {
        LookupAccountSidW(
            ptr::null(),
            raw,
            ptr::null_mut(),
            &mut name_len,
            ptr::null_mut(),
            &mut domain_len,
            &mut kind,
        );
    }
    if io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
        return Err(io::Error::last_os_error().into());
    }
    let mut name = vec![0; name_len as usize];
    let mut domain = vec![0; domain_len as usize];
    // SAFETY: buffers have the measured capacities.
    if unsafe {
        LookupAccountSidW(
            ptr::null(),
            raw,
            name.as_mut_ptr(),
            &mut name_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut kind,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    if kind != SidTypeUser {
        bail!("a node must belong to a user account, not {sid}");
    }
    name.truncate(name_len as usize);
    domain.truncate(domain_len as usize);
    Ok(Target { sid, user: name, domain })
}

pub(super) fn enable_privileges() -> anyhow::Result<()> {
    let token = current_token(TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES)?;
    for privilege in [
        "SeTcbPrivilege",
        "SeAssignPrimaryTokenPrivilege",
        "SeIncreaseQuotaPrivilege",
        "SeBackupPrivilege",
        "SeRestorePrivilege",
    ] {
        let name: Vec<u16> = privilege.encode_utf16().chain([0]).collect();
        let mut luid = LUID::default();
        // SAFETY: a terminated privilege name, writable output and valid token.
        unsafe {
            if LookupPrivilegeValueW(ptr::null(), name.as_ptr(), &mut luid) == 0 {
                return Err(io::Error::last_os_error().into());
            }
            let change = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
            };
            SetLastError(ERROR_SUCCESS);
            if AdjustTokenPrivileges(token.as_raw_handle(), 0, &change, 0, ptr::null_mut(), ptr::null_mut()) == 0
                || io::Error::last_os_error().raw_os_error() != Some(0)
            {
                return Err(io::Error::last_os_error()).with_context(|| format!("enabling {privilege}"));
            }
        }
    }
    Ok(())
}

/// Prefer the already-used full logon when several sessions belong to this
/// account. Switching to somebody else's desktop does not change our user.
pub(super) fn interactive(
    target: &Target,
    current: Option<Identity>,
) -> anyhow::Result<Option<(Identity, OwnedHandle)>> {
    let mut sessions = ptr::null_mut();
    let mut count = 0;
    // SAFETY: Windows allocates the session array; no session metadata beyond
    // the token's account SID and AuthenticationId is inspected.
    if unsafe { WTSEnumerateSessionsW(WTS_CURRENT_SERVER_HANDLE, 0, 1, &mut sessions, &mut count) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    let result = (|| {
        let mut chosen = None;
        if count == 0 {
            return Ok(None);
        }
        // SAFETY: the array contains count entries until WTSFreeMemory below.
        for session in unsafe { std::slice::from_raw_parts(sessions, count as usize) } {
            // Listener and initialization entries are not user logons.
            if [WTSListen, WTSDown, WTSInit].contains(&session.State) {
                continue;
            }
            let mut raw = ptr::null_mut();
            if unsafe { WTSQueryUserToken(session.SessionId, &mut raw) } == 0 {
                let error = io::Error::last_os_error();
                // 7022 is ERROR_CTX_WINSTATION_NOT_FOUND: the session ended
                // between enumeration and this query, like ERROR_NO_TOKEN.
                if matches!(error.raw_os_error(), Some(code) if code == ERROR_NO_TOKEN as i32 || code == 7022) {
                    continue;
                }
                return Err(error)
                    .with_context(|| format!("querying the user token of WTS session {}", session.SessionId));
            }
            // SAFETY: WTSQueryUserToken returned a new owned token.
            let token = unsafe { OwnedHandle::from_raw_handle(raw) };
            if token_sid(raw)? != target.sid {
                continue;
            }
            let identity = Identity::Interactive(authentication_id(raw)?);
            if Some(identity) == current {
                return Ok(Some((identity, token)));
            }
            if chosen.is_none() {
                chosen = Some((identity, token));
            }
        }
        Ok(chosen)
    })();
    // SAFETY: the session array was allocated by WTS.
    unsafe {
        WTSFreeMemory(sessions.cast());
    }
    result
}

pub(super) fn login(target: &Target, full: Option<(Identity, OwnedHandle)>) -> anyhow::Result<Login> {
    let (identity, token) = match full {
        Some((identity, original)) => {
            // Where User Account Control handed the desktop logon a limited
            // token, give the node the account's full, elevated one instead
            // (see elevated_counterpart); otherwise the token Windows gave.
            let elevated = elevated_counterpart(original.as_raw_handle());
            let base = elevated.as_ref().unwrap_or(&original);
            let mut duplicate = ptr::null_mut();
            // SAFETY: duplicates the chosen primary token without changing
            // its authentication session or asking for UAC elevation.
            if unsafe {
                DuplicateTokenEx(
                    base.as_raw_handle(),
                    TOKEN_ALL_ACCESS,
                    ptr::null(),
                    SecurityImpersonation,
                    TokenPrimary,
                    &mut duplicate,
                )
            } == 0
            {
                return Err(io::Error::last_os_error().into());
            }
            (identity, unsafe { OwnedHandle::from_raw_handle(duplicate) })
        }
        // A logon with nobody at the desktop is given the full token too,
        // where Windows handed over the limited half.
        None => {
            let token = s4u(target)?;
            (Identity::S4u, elevated_counterpart(token.as_raw_handle()).unwrap_or(token))
        }
    };
    if token_sid(token.as_raw_handle())? != target.sid {
        bail!("Windows returned a token for a different account");
    }
    let before = authentication_id(token.as_raw_handle())?;
    let session: u32 = 0;
    // SAFETY: session 0 keeps the node out of desktop logoff teardown. This
    // changes a WTS session, not the LSA authentication session.
    if unsafe {
        SetTokenInformation(token.as_raw_handle(), TokenSessionId, (&raw const session).cast(), size_of::<u32>() as u32)
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    if authentication_id(token.as_raw_handle())? != before {
        bail!("moving the token changed its authentication session");
    }
    let mut user = target.user.clone();
    user.push(0);
    let mut profile = PROFILEINFOW {
        dwSize: size_of::<PROFILEINFOW>() as u32,
        dwFlags: 1,
        lpUserName: user.as_mut_ptr(),
        ..Default::default()
    };
    // SAFETY: user stays alive for the call; PI_NOUI suppresses profile UI.
    if unsafe { LoadUserProfileW(token.as_raw_handle(), &mut profile) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    let mut length = 0;
    unsafe {
        GetUserProfileDirectoryW(token.as_raw_handle(), ptr::null_mut(), &mut length);
    }
    let mut home = vec![0; length as usize];
    if unsafe { GetUserProfileDirectoryW(token.as_raw_handle(), home.as_mut_ptr(), &mut length) } == 0 {
        let error = io::Error::last_os_error();
        unsafe {
            UnloadUserProfile(token.as_raw_handle(), profile.hProfile);
        }
        return Err(error.into());
    }
    let end = home.iter().position(|&c| c == 0).unwrap_or(home.len());
    Ok(Login { token, identity, home: OsString::from_wide(&home[..end]), profile: profile.hProfile })
}

fn s4u(target: &Target) -> anyhow::Result<OwnedHandle> {
    let mut source_name = *b"fastctx\0";
    let source_string = lsa_string(&mut source_name);
    let mut lsa = ptr::null_mut();
    let mut mode = 0;
    // SAFETY: all input/output buffers outlive each LSA call. Profile data
    // is freed without inspecting any authentication-package contents.
    unsafe {
        status(LsaRegisterLogonProcess(&source_string, &mut lsa, &mut mode))?;
        let result = (|| {
            let mut package_name = *b"MICROSOFT_AUTHENTICATION_PACKAGE_V1_0\0";
            let mut package = 0;
            status(LsaLookupAuthenticationPackage(lsa, &lsa_string(&mut package_name), &mut package))?;
            let bytes = size_of::<MSV1_0_S4U_LOGON>() + (target.user.len() + target.domain.len() + 2) * 2;
            let mut buffer = vec![0usize; bytes.div_ceil(size_of::<usize>())];
            let logon = buffer.as_mut_ptr().cast::<MSV1_0_S4U_LOGON>();
            let mut cursor = buffer.as_mut_ptr().cast::<u8>().add(size_of::<MSV1_0_S4U_LOGON>()).cast::<u16>();
            let mut string = |text: &[u16]| -> anyhow::Result<LSA_UNICODE_STRING> {
                ptr::copy_nonoverlapping(text.as_ptr(), cursor, text.len());
                let value = LSA_UNICODE_STRING {
                    Length: u16::try_from(text.len() * 2)?,
                    MaximumLength: u16::try_from((text.len() + 1) * 2)?,
                    Buffer: cursor,
                };
                cursor = cursor.add(text.len() + 1);
                Ok(value)
            };
            *logon = MSV1_0_S4U_LOGON {
                MessageType: MsV1_0S4ULogon,
                Flags: 0,
                UserPrincipalName: string(&target.user)?,
                DomainName: string(&target.domain)?,
            };
            let mut source = TOKEN_SOURCE { SourceName: b"fastctx\0".map(|c| c as i8), ..Default::default() };
            if AllocateLocallyUniqueId(&mut source.SourceIdentifier) == 0 {
                return Err(io::Error::last_os_error().into());
            }
            let mut profile = ptr::null_mut();
            let mut profile_len = 0;
            let mut id = LUID::default();
            let mut token = ptr::null_mut();
            let mut quotas: QUOTA_LIMITS = zeroed();
            let mut substatus = 0;
            let result = LsaLogonUser(
                lsa,
                &source_string,
                SECURITY_LOGON_TYPE::Batch,
                package,
                logon.cast(),
                bytes.try_into()?,
                ptr::null(),
                &source,
                &mut profile,
                &mut profile_len,
                &mut id,
                &mut token,
                &mut quotas,
                &mut substatus,
            );
            if !profile.is_null() {
                LsaFreeReturnBuffer(profile);
            }
            status(result).with_context(|| format!("S4U batch logon for {}; substatus {substatus:#x}", target.sid))?;
            Ok(OwnedHandle::from_raw_handle(token))
        })();
        LsaDeregisterLogonProcess(lsa);
        result
    }
}

fn lsa_string(value: &mut [u8]) -> LSA_STRING {
    LSA_STRING { Length: (value.len() - 1) as u16, MaximumLength: value.len() as u16, Buffer: value.as_mut_ptr() }
}

fn status(code: i32) -> anyhow::Result<()> {
    if code < 0 {
        Err(io::Error::from_raw_os_error(unsafe { LsaNtStatusToWinError(code) } as i32).into())
    } else {
        Ok(())
    }
}

fn current_token(access: u32) -> anyhow::Result<OwnedHandle> {
    let mut token = ptr::null_mut();
    // SAFETY: queries this process and returns an owned token handle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut token) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(token) })
}

fn token_info(token: HANDLE, class: i32) -> anyhow::Result<Vec<usize>> {
    let mut needed = 0;
    // SAFETY: query the exact buffer size, then supply aligned storage.
    unsafe {
        GetTokenInformation(token, class, ptr::null_mut(), 0, &mut needed);
    }
    if io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
        return Err(io::Error::last_os_error().into());
    }
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    if unsafe { GetTokenInformation(token, class, buffer.as_mut_ptr().cast(), needed, &mut needed) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(buffer)
}

/// The account's full, elevated token where the one Windows hands a desktop
/// logon is the limited half User Account Control splits off for an
/// administrator; `None` for a standard user, who has no such token, and for
/// a token that is already full. The guardian runs as SYSTEM and holds
/// SeTcbPrivilege and SeAssignPrimaryTokenPrivilege, so starting the node
/// from this token needs no User Account Control prompt: the node, and every
/// job `run` starts under it, then holds this machine's own administrator
/// rights, which is what an owner's agent is meant to have.
fn elevated_counterpart(token: HANDLE) -> Option<OwnedHandle> {
    let mut kind = 0i32;
    let mut needed = 0;
    // SAFETY: the buffer is a TOKEN_ELEVATION_TYPE (an i32) of the size given.
    let known = unsafe {
        GetTokenInformation(token, TokenElevationType, (&raw mut kind).cast(), size_of::<i32>() as u32, &mut needed)
    };
    if known == 0 || kind != TokenElevationTypeLimited {
        return None;
    }
    let mut linked = TOKEN_LINKED_TOKEN { LinkedToken: ptr::null_mut() };
    // SAFETY: the buffer is a TOKEN_LINKED_TOKEN of the size given; on success
    // it owns a new token handle, taken over by OwnedHandle below.
    let got = unsafe {
        GetTokenInformation(
            token,
            TokenLinkedToken,
            (&raw mut linked).cast(),
            size_of::<TOKEN_LINKED_TOKEN>() as u32,
            &mut needed,
        )
    };
    if got == 0 {
        return None;
    }
    // SAFETY: GetTokenInformation handed over ownership of this handle.
    Some(unsafe { OwnedHandle::from_raw_handle(linked.LinkedToken) })
}

fn token_sid(token: HANDLE) -> anyhow::Result<String> {
    let buffer = token_info(token, TokenUser)?;
    sid_text(unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid })
}

fn authentication_id(token: HANDLE) -> anyhow::Result<u64> {
    let buffer = token_info(token, TokenStatistics)?;
    let id = unsafe { (*buffer.as_ptr().cast::<TOKEN_STATISTICS>()).AuthenticationId };
    Ok((u64::from(id.HighPart as u32) << 32) | u64::from(id.LowPart))
}

fn sid_text(sid: *mut std::ffi::c_void) -> anyhow::Result<String> {
    let mut text = ptr::null_mut();
    // SAFETY: callers pass a live SID; LocalFree releases the returned text.
    unsafe {
        if ConvertSidToStringSidW(sid, &mut text) == 0 {
            return Err(io::Error::last_os_error().into());
        }
        let length = (0..).take_while(|&index| *text.add(index) != 0).count();
        let value = String::from_utf16(std::slice::from_raw_parts(text, length));
        LocalFree(text.cast());
        Ok(value?)
    }
}
