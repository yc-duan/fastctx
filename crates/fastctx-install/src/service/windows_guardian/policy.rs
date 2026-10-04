//! Batch logon is a right of the account, not of the SYSTEM caller. A failed
//! installation restores its previous direct grant. A committed grant stays
//! with the account: other services and tasks may subsequently depend on it.

use std::{io, mem::size_of, ptr};

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, LocalFree};
use windows_sys::Win32::Security::Authentication::Identity::{
    LSA_HANDLE, LSA_OBJECT_ATTRIBUTES, LSA_UNICODE_STRING, LsaAddAccountRights, LsaClose, LsaEnumerateAccountRights,
    LsaFreeMemory, LsaNtStatusToWinError, LsaOpenPolicy, LsaRemoveAccountRights, POLICY_CREATE_ACCOUNT,
    POLICY_LOOKUP_NAMES,
};
use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;

struct Account {
    policy: LSA_HANDLE,
    sid: *mut std::ffi::c_void,
}

impl Account {
    fn open(sid: &str) -> anyhow::Result<Self> {
        let value: Vec<u16> = sid.encode_utf16().chain([0]).collect();
        let mut raw = ptr::null_mut();
        // SAFETY: this is the normalized SID returned by account lookup.
        if unsafe { ConvertStringSidToSidW(value.as_ptr(), &mut raw) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        let mut account = Self { policy: 0, sid: raw };
        let attributes =
            LSA_OBJECT_ATTRIBUTES { Length: size_of::<LSA_OBJECT_ATTRIBUTES>() as u32, ..Default::default() };
        check(unsafe {
            LsaOpenPolicy(
                ptr::null(),
                &attributes,
                (POLICY_LOOKUP_NAMES | POLICY_CREATE_ACCOUNT) as u32,
                &mut account.policy,
            )
        })?;
        Ok(account)
    }
}

impl Drop for Account {
    fn drop(&mut self) {
        unsafe {
            if self.policy != 0 {
                LsaClose(self.policy);
            }
            LocalFree(self.sid);
        }
    }
}

pub(super) fn batch_granted(sid: &str) -> anyhow::Result<bool> {
    let account = Account::open(sid)?;
    let mut rights = ptr::null_mut();
    let mut count = 0;
    let result = unsafe { LsaEnumerateAccountRights(account.policy, account.sid, &mut rights, &mut count) };
    if unsafe { LsaNtStatusToWinError(result) } == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    check(result)?;
    let wanted: Vec<u16> = "SeBatchLogonRight".encode_utf16().collect();
    let granted = count != 0
        && unsafe { std::slice::from_raw_parts(rights, count as usize) }
            .iter()
            .any(|right| (unsafe { std::slice::from_raw_parts(right.Buffer, right.Length as usize / 2) }) == wanted);
    unsafe {
        LsaFreeMemory(rights.cast());
    }
    Ok(granted)
}

pub(super) fn set_batch(sid: &str, granted: bool) -> anyhow::Result<()> {
    if batch_granted(sid)? == granted {
        return Ok(());
    }
    let account = Account::open(sid)?;
    let mut text: Vec<u16> = "SeBatchLogonRight".encode_utf16().collect();
    let right = LSA_UNICODE_STRING {
        Length: (text.len() * 2) as u16,
        MaximumLength: (text.len() * 2) as u16,
        Buffer: text.as_mut_ptr(),
    };
    // Never edit a deny rule or any unrelated user right.
    check(unsafe {
        if granted {
            LsaAddAccountRights(account.policy, account.sid, &right, 1)
        } else {
            LsaRemoveAccountRights(account.policy, account.sid, false, &right, 1)
        }
    })
}

fn check(status: i32) -> anyhow::Result<()> {
    if status < 0 {
        Err(io::Error::from_raw_os_error(unsafe { LsaNtStatusToWinError(status) } as i32).into())
    } else {
        Ok(())
    }
}
