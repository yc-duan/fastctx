//! The account 1.0.1-beta.3 made for a managed Windows computer installed
//! from its line by SYSTEM (`world-network.md`, 登录与入网): a local
//! administrator named `fastctx`, hidden from the sign-in screen, whose
//! password nobody knows. A computer installed so keeps running in it; a
//! line run by SYSTEM now makes the node SYSTEM's own, and such an account
//! is taken away once no World is kept in it.

use std::ffi::OsStr;
use std::io;
use std::ptr;

use anyhow::{Context as _, bail};
use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, LocalFree};
use windows_sys::Win32::NetworkManagement::NetManagement::{
    NERR_Success, NERR_UserNotFound, NetApiBufferFree, NetUserDel, NetUserGetInfo, USER_INFO_1,
};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{LookupAccountNameW, SID_NAME_USE, SidTypeUser};
use windows_sys::Win32::UI::Shell::DeleteProfileW;

use super::child::wide;

/// The account a managed Windows computer's World lived in.
pub const ACCOUNT: &str = "fastctx";
/// What its description says, by which Fastctx knows it for its own.
const MARK: &str = "Fastctx: the World of this managed machine";
/// Where Windows lists the accounts its sign-in screen does not show.
const HIDDEN: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon\SpecialAccounts\UserList";

/// The SID of the account, when one Fastctx made is on this computer.
pub fn made() -> anyhow::Result<Option<String>> {
    let name = wide(OsStr::new(ACCOUNT))?;
    if described(&name)?.flatten().as_deref() != Some(MARK) {
        return Ok(None);
    }
    Ok(Some(sid_of(&name)?))
}

/// Whether a node service still keeps the World of the account `sid`.
pub fn keeps_a_world(sid: &str) -> anyhow::Result<bool> {
    let entries = match std::fs::read_dir(super::security::services()?) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if super::security::validate_name(&name).is_ok()
            && let Ok(config) = super::config::Config::read(&name)
            && config.target.is_some_and(|target| target.sid == sid)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Takes the account away with what was given to it: its right of a batch
/// logon, its place off the sign-in screen, and its profile when nothing
/// holds that any more. What could not go, said as a person can find it.
pub fn remove(sid: &str) -> anyhow::Result<Vec<String>> {
    if made()?.as_deref() != Some(sid) {
        bail!("{sid} is not the account Fastctx made; it is left as it is");
    }
    let mut left = Vec::new();
    super::policy::set_batch(sid, false).context("withdrawing the account's batch logon")?;
    let name = wide(OsStr::new(ACCOUNT))?;
    // SAFETY: a terminated account name on this computer.
    let code = unsafe { NetUserDel(ptr::null(), name.as_ptr()) };
    if code != NERR_Success && code != NERR_UserNotFound {
        bail!("removing the account {ACCOUNT}: {}", io::Error::from_raw_os_error(code as i32));
    }
    const NOT_FOUND: i32 = 0x80070002u32 as i32;
    if let Err(error) = windows_registry::LOCAL_MACHINE.create(HIDDEN).and_then(|key| key.remove_value(ACCOUNT))
        && error.code().0 != NOT_FOUND
    {
        left.push(format!("the sign-in screen's entry for {ACCOUNT}: {error}"));
    }
    let text = wide(OsStr::new(sid))?;
    // SAFETY: a terminated SID string; no profile path or computer is named.
    if unsafe { DeleteProfileW(text.as_ptr(), ptr::null(), ptr::null()) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(2) {
            left.push(format!("the profile of {ACCOUNT} ({error}); delete it once the computer has restarted"));
        }
    }
    Ok(left)
}

/// The account's description: none when the account does not exist,
/// `Some(None)` when it has none.
fn described(name: &[u16]) -> anyhow::Result<Option<Option<String>>> {
    let mut buffer = ptr::null_mut();
    // SAFETY: level 1 returns a USER_INFO_1 freed with NetApiBufferFree.
    let code = unsafe { NetUserGetInfo(ptr::null(), name.as_ptr(), 1, &mut buffer) };
    if code == NERR_UserNotFound {
        return Ok(None);
    }
    if code != NERR_Success {
        bail!("reading the account {ACCOUNT}: {}", io::Error::from_raw_os_error(code as i32));
    }
    // SAFETY: the buffer holds one USER_INFO_1 until it is freed below.
    let info = unsafe { &*buffer.cast::<USER_INFO_1>() };
    let comment = (!info.usri1_comment.is_null()).then(|| {
        // SAFETY: Windows returned a terminated string inside the buffer.
        let length = (0..).take_while(|&at| unsafe { *info.usri1_comment.add(at) } != 0).count();
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(info.usri1_comment, length) })
    });
    unsafe {
        NetApiBufferFree(buffer.cast());
    }
    Ok(Some(comment))
}

fn sid_of(name: &[u16]) -> anyhow::Result<String> {
    let (mut sid_len, mut domain_len, mut kind): (u32, u32, SID_NAME_USE) = (0, 0, 0);
    // SAFETY: the first call measures the two buffers.
    unsafe {
        LookupAccountNameW(
            ptr::null(),
            name.as_ptr(),
            ptr::null_mut(),
            &mut sid_len,
            ptr::null_mut(),
            &mut domain_len,
            &mut kind,
        );
    }
    if io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
        return Err(io::Error::last_os_error()).with_context(|| format!("finding the account {ACCOUNT}"));
    }
    let mut sid = vec![0u8; sid_len as usize];
    let mut domain = vec![0u16; domain_len as usize];
    // SAFETY: buffers sized by the measuring call above.
    if unsafe {
        LookupAccountNameW(
            ptr::null(),
            name.as_ptr(),
            sid.as_mut_ptr().cast(),
            &mut sid_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut kind,
        )
    } == 0
    {
        return Err(io::Error::last_os_error()).with_context(|| format!("finding the account {ACCOUNT}"));
    }
    if kind != SidTypeUser {
        bail!("{ACCOUNT} names something other than a user account");
    }
    let mut text = ptr::null_mut();
    // SAFETY: a valid SID in `sid`; the string is freed below.
    if unsafe { ConvertSidToStringSidW(sid.as_mut_ptr().cast(), &mut text) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    let length = (0..).take_while(|&at| unsafe { *text.add(at) } != 0).count();
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe {
        LocalFree(text.cast());
    }
    Ok(value)
}
