//! Find an existing interactive session of this exact user without requiring
//! SYSTEM's WTSQueryUserToken privilege.

use anyhow::{Context as _, bail};
use windows_sys::Win32::Foundation::{GetLastError, LocalFree};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::LookupAccountNameW;
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::RemoteDesktop::{
    WTS_CURRENT_SERVER_HANDLE, WTS_INFO_CLASS, WTSActive, WTSDomainName, WTSEnumerateSessionsW, WTSFreeMemory,
    WTSQuerySessionInformationW, WTSUserName,
};

pub fn current() -> anyhow::Result<(String, u32)> {
    let sid = fastctx_platform::current_user_sid().context("reading interactive repair user SID")?;
    let mut current = 0;
    if unsafe { ProcessIdToSessionId(std::process::id(), &mut current) } == 0 {
        return Err(std::io::Error::last_os_error()).context("reading repair process session");
    }
    let mut sessions = std::ptr::null_mut();
    let mut count = 0;
    if unsafe { WTSEnumerateSessionsW(WTS_CURRENT_SERVER_HANDLE, 0, 1, &mut sessions, &mut count) } == 0 {
        return Err(std::io::Error::last_os_error()).context("enumerating interactive Windows sessions");
    }
    let entries = if count == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(sessions, count as usize) } };
    let candidates: Vec<_> =
        entries.iter().filter(|s| s.SessionId > 0 && s.State == WTSActive).map(|s| s.SessionId).collect();
    unsafe {
        WTSFreeMemory(sessions.cast());
    }
    if candidates.contains(&current) {
        return Ok((sid, current));
    }
    let mut errors = Vec::new();
    for session in candidates {
        match user(session) {
            Ok(Some(user)) if user == sid => return Ok((sid, session)),
            Ok(_) => (),
            Err(error) => errors.push(format!("session {session}: {error:#}")),
        }
    }
    if errors.is_empty() {
        bail!("user {sid} has no active interactive Windows session; application repair will retry after sign-in");
    }
    bail!("finding an interactive Windows session for {sid}: {}", errors.join("; "))
}

fn text(session: u32, kind: WTS_INFO_CLASS) -> anyhow::Result<String> {
    let mut buffer = std::ptr::null_mut();
    let mut bytes = 0;
    if unsafe { WTSQuerySessionInformationW(WTS_CURRENT_SERVER_HANDLE, session, kind, &mut buffer, &mut bytes) } == 0 {
        return Err(std::io::Error::last_os_error()).context("querying session account");
    }
    let result = if bytes == 0 {
        String::new()
    } else {
        let value = unsafe { std::slice::from_raw_parts(buffer, bytes as usize / 2) };
        String::from_utf16_lossy(value.split(|&c| c == 0).next().unwrap_or_default())
    };
    unsafe {
        WTSFreeMemory(buffer.cast());
    }
    Ok(result)
}

fn user(session: u32) -> anyhow::Result<Option<String>> {
    let name = text(session, WTSUserName)?;
    if name.is_empty() {
        return Ok(None);
    }
    let domain = text(session, WTSDomainName)?;
    let account = if domain.is_empty() { name } else { format!("{domain}\\{name}") };
    let wide: Vec<_> = account.encode_utf16().chain([0]).collect();
    let (mut sid_bytes, mut domain_chars, mut kind) = (0, 0, 0);
    unsafe {
        LookupAccountNameW(
            std::ptr::null(),
            wide.as_ptr(),
            std::ptr::null_mut(),
            &mut sid_bytes,
            std::ptr::null_mut(),
            &mut domain_chars,
            &mut kind,
        );
    }
    if sid_bytes == 0 {
        bail!("resolving session account {account}: Windows error {}", unsafe { GetLastError() });
    }
    let mut sid = vec![0u8; sid_bytes as usize];
    let mut domain = vec![0u16; domain_chars as usize];
    if unsafe {
        LookupAccountNameW(
            std::ptr::null(),
            wide.as_ptr(),
            sid.as_mut_ptr().cast(),
            &mut sid_bytes,
            domain.as_mut_ptr(),
            &mut domain_chars,
            &mut kind,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error()).with_context(|| format!("resolving session account {account}"));
    }
    let mut value = std::ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid.as_mut_ptr().cast(), &mut value) } == 0 {
        return Err(std::io::Error::last_os_error()).context("formatting session account SID");
    }
    let mut length = 0;
    unsafe {
        while *value.add(length) != 0 {
            length += 1;
        }
    }
    let result = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(value, length) });
    unsafe {
        LocalFree(value.cast());
    }
    Ok(Some(result))
}
