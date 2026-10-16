//! A Windows computer installed from its line by SYSTEM, as a remote
//! management tool runs it, lives in an account of its own
//! (`world-network.md`, 受管机器): a local administrator nobody signs in as,
//! hidden from the sign-in screen, whose password nobody knows. Its World is
//! that account's: what belongs to it is done as it, through the same logon
//! the guardian starts its node with; only what takes an administrator is
//! done as SYSTEM.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::mem::{size_of, zeroed};
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr;

use anyhow::{Context as _, bail};
use windows_sys::Win32::Foundation::SetHandleInformation;
use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_MEMBER_IN_ALIAS, HANDLE_FLAG_INHERIT, LocalFree,
};
use windows_sys::Win32::NetworkManagement::NetManagement::{
    LOCALGROUP_MEMBERS_INFO_3, NERR_Success, NERR_UserExists, NetApiBufferFree, NetLocalGroupAddMembers, NetUserAdd,
    NetUserGetInfo, NetUserSetInfo, UF_ACCOUNTDISABLE, UF_DONT_EXPIRE_PASSWD, UF_PASSWD_CANT_CHANGE, UF_SCRIPT,
    USER_INFO_1, USER_INFO_1008, USER_PRIV_USER,
};
use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSidToSidW};
use windows_sys::Win32::Security::{LookupAccountNameW, LookupAccountSidW, SID_NAME_USE, SidTypeUser};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, EXTENDED_STARTUPINFO_PRESENT,
    GetExitCodeProcess, INFINITE, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, WaitForSingleObject,
};

use super::account::{self, Environment};
use super::child::{Attributes, command_line, private_pipe, wide};

/// The account a managed Windows computer's World lives in.
pub const ACCOUNT: &str = "fastctx";
/// What its description says, by which a later run knows it for Fastctx's.
const MARK: &str = "Fastctx: the World of this managed machine";
/// Where Windows lists the accounts its sign-in screen does not show.
const HIDDEN: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon\SpecialAccounts\UserList";
/// The well-known SID of the local Administrators group.
const ADMINISTRATORS: &str = "S-1-5-32-544";

/// Makes the account, or finds the one an earlier run made, and gives it
/// what its World needs: membership of Administrators, a place off the
/// sign-in screen and the right of a batch logon. Its SID. An account of
/// that name that is not Fastctx's is left alone, and said so.
pub fn account() -> anyhow::Result<String> {
    let name = wide(OsStr::new(ACCOUNT))?;
    let mut comment = wide(OsStr::new(MARK))?;
    let mut password = wide(OsStr::new(&password()?))?;
    let mut account_name = name.clone();
    let info = USER_INFO_1 {
        usri1_name: account_name.as_mut_ptr(),
        usri1_password: password.as_mut_ptr(),
        usri1_priv: USER_PRIV_USER,
        usri1_comment: comment.as_mut_ptr(),
        usri1_flags: UF_SCRIPT | UF_DONT_EXPIRE_PASSWD | UF_PASSWD_CANT_CHANGE,
        ..Default::default()
    };
    let mut wrong = 0;
    // SAFETY: every string outlives the call; level 1 matches USER_INFO_1.
    let added = unsafe { NetUserAdd(ptr::null(), 1, (&raw const info).cast(), &mut wrong) };
    password.iter_mut().for_each(|unit| *unit = 0);
    if added == NERR_UserExists {
        if described(&name)?.as_deref() != Some(MARK) {
            bail!(
                "this computer has an account named {ACCOUNT} that Fastctx did not make; it is left as it is, and Fastctx is not installed"
            );
        }
        enable(&name)?;
    } else if added != NERR_Success {
        bail!("making the account {ACCOUNT}: {} (parameter {wrong})", io::Error::from_raw_os_error(added as i32));
    }
    administrator(&name)?;
    windows_registry::LOCAL_MACHINE
        .create(HIDDEN)
        .and_then(|key| key.set_u32(ACCOUNT, 0))
        .with_context(|| format!("hiding {ACCOUNT} from the sign-in screen"))?;
    let sid = sid_of(&name)?;
    super::policy::set_batch(&sid, true).context("granting the account its batch logon")?;
    Ok(sid)
}

/// Runs `program` with `args` as the account `sid`, through the logon its
/// node runs with: its own profile, environment and home. What it writes
/// comes out here as it comes; its exit code, and the account's home.
pub fn run(sid: &str, program: &Path, args: &[OsString]) -> anyhow::Result<(u32, PathBuf)> {
    account::enable_privileges()?;
    let target = account::target(Some(sid))?;
    let login = account::login(&target, None).context("signing the account in")?;
    let token = login.token.as_raw_handle();
    let environment = Environment::new(Some(token)).context("creating the account's environment block")?;
    let (mut read, write) = private_pipe(Some(sid))?;
    let input = File::open(r"\\.\NUL").context("opening null stdin")?;
    let handles = [input.as_raw_handle(), write.as_raw_handle()];
    for &handle in &handles {
        // SAFETY: only the listed child ends are inheritable, and only by it.
        if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
    }
    let mut attributes = Attributes::new()?;
    attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize, &handles)?;
    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = input.as_raw_handle();
    startup.StartupInfo.hStdOutput = write.as_raw_handle();
    startup.StartupInfo.hStdError = write.as_raw_handle();
    startup.lpAttributeList = attributes.pointer();
    let executable = wide(program.as_os_str())?;
    let mut command = command_line(program.as_os_str(), args)?;
    let home = wide(login.home.as_os_str())?;
    let mut info: PROCESS_INFORMATION = unsafe { zeroed() };
    // SAFETY: every pointer and listed handle lives through creation.
    if unsafe {
        CreateProcessAsUserW(
            token,
            executable.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            environment.as_ptr(),
            home.as_ptr(),
            &startup.StartupInfo,
            &mut info,
        )
    } == 0
    {
        return Err(io::Error::last_os_error()).with_context(|| format!("starting {} as {ACCOUNT}", program.display()));
    }
    // SAFETY: CreateProcessAsUserW returned these two new owned handles.
    let (process, _thread) =
        unsafe { (OwnedHandle::from_raw_handle(info.hProcess), OwnedHandle::from_raw_handle(info.hThread)) };
    // Ours closed, the pipe ends when the child and what it started let go.
    drop(write);
    drop(input);
    let mut out = io::stdout();
    let mut buffer = [0; 4096];
    loop {
        match read.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                let _ = out.write_all(&buffer[..count]);
                let _ = out.flush();
            }
        }
    }
    // SAFETY: a live process handle; INFINITE waits for its end.
    unsafe {
        WaitForSingleObject(process.as_raw_handle(), INFINITE);
    }
    let mut code = 0;
    // SAFETY: the process has ended; its exit code is readable.
    if unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok((code, PathBuf::from(login.home.clone())))
}

/// A long password nobody is told, of every kind a password policy asks.
fn password() -> anyhow::Result<String> {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).map_err(|error| anyhow::anyhow!("making a password: {error}"))?;
    const LETTERS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut password: String = bytes.iter().map(|byte| LETTERS[usize::from(*byte) % LETTERS.len()] as char).collect();
    password.push_str("Aa9#");
    Ok(password)
}

/// The account's description, or none when it has none.
fn described(name: &[u16]) -> anyhow::Result<Option<String>> {
    let mut buffer = ptr::null_mut();
    // SAFETY: level 1 returns a USER_INFO_1 freed with NetApiBufferFree.
    let code = unsafe { NetUserGetInfo(ptr::null(), name.as_ptr(), 1, &mut buffer) };
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
    Ok(comment)
}

/// The account enabled, as an earlier run made it, whatever changed since.
fn enable(name: &[u16]) -> anyhow::Result<()> {
    let flags = USER_INFO_1008 { usri1008_flags: UF_SCRIPT | UF_DONT_EXPIRE_PASSWD | UF_PASSWD_CANT_CHANGE };
    debug_assert_eq!(flags.usri1008_flags & UF_ACCOUNTDISABLE, 0);
    let mut wrong = 0;
    // SAFETY: level 1008 matches USER_INFO_1008.
    let code = unsafe { NetUserSetInfo(ptr::null(), name.as_ptr(), 1008, (&raw const flags).cast(), &mut wrong) };
    if code != NERR_Success {
        bail!("enabling the account {ACCOUNT}: {}", io::Error::from_raw_os_error(code as i32));
    }
    Ok(())
}

/// The account a member of the local Administrators group, by its SID:
/// the group's name is in the language of the system.
fn administrator(name: &[u16]) -> anyhow::Result<()> {
    let group = wide(OsStr::new(&group_name(ADMINISTRATORS)?))?;
    let mut member = name.to_vec();
    let entry = LOCALGROUP_MEMBERS_INFO_3 { lgrmi3_domainandname: member.as_mut_ptr() };
    // SAFETY: level 3 matches one LOCALGROUP_MEMBERS_INFO_3.
    let code = unsafe { NetLocalGroupAddMembers(ptr::null(), group.as_ptr(), 3, (&raw const entry).cast(), 1) };
    if code != NERR_Success && code != ERROR_MEMBER_IN_ALIAS {
        bail!("making {ACCOUNT} an administrator: {}", io::Error::from_raw_os_error(code as i32));
    }
    Ok(())
}

fn group_name(sid: &str) -> anyhow::Result<String> {
    let text = wide(OsStr::new(sid))?;
    let mut raw = ptr::null_mut();
    // SAFETY: a terminated SID string; the SID is freed below.
    if unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut raw) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    let (mut name_len, mut domain_len, mut kind): (u32, u32, SID_NAME_USE) = (0, 0, 0);
    let looked = (|| {
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
        let mut name = vec![0u16; name_len as usize];
        let mut domain = vec![0u16; domain_len as usize];
        // SAFETY: buffers sized by the measuring call above.
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
            return Err(io::Error::last_os_error());
        }
        name.truncate(name_len as usize);
        Ok(String::from_utf16_lossy(&name))
    })();
    unsafe {
        LocalFree(raw);
    }
    looked.context("naming the local Administrators group")
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
