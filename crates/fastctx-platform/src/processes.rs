//! The processes running on this machine as a read-only look lists them for
//! the World's own agents (`world-senses.md`, 哨兵是世界自己的 agent): each
//! one's id, name, command line where this user may read it, when it
//! started and the memory it holds. Nothing is spawned to learn it.

use std::io;
use std::time::SystemTime;

/// One process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Running {
    pub pid: u32,
    pub name: String,
    /// Its command line, where this user may read it.
    pub command: Option<String>,
    pub started: Option<SystemTime>,
    /// The memory it holds now, in bytes.
    pub memory: Option<u64>,
}

/// Every process now that this user may see; one that exits while the list
/// is read is left out.
pub fn running() -> io::Result<Vec<Running>> {
    imp::running()
}

#[cfg(target_os = "linux")]
mod imp {
    use super::Running;
    use std::fs;
    use std::io;
    use std::time::{Duration, SystemTime};

    pub(super) fn running() -> io::Result<Vec<Running>> {
        // SAFETY: sysconf only reads the system's configuration.
        let (ticks, page) = unsafe { (libc::sysconf(libc::_SC_CLK_TCK), libc::sysconf(libc::_SC_PAGESIZE)) };
        let boot = boot();
        let mut found = Vec::new();
        for stat in crate::procfs::all()? {
            // Kernel threads are kthreadd's children and run no program.
            if stat.has_exited() || stat.pid == 2 || stat.ppid == 2 {
                continue;
            }
            let pid = stat.pid;
            let name = fs::read_to_string(format!("/proc/{pid}/comm"))
                .map(|name| name.trim_end().to_string())
                .unwrap_or_default();
            let command = fs::read(format!("/proc/{pid}/cmdline")).ok().and_then(|line| {
                let words: Vec<_> = line
                    .split(|&byte| byte == 0)
                    .filter(|word| !word.is_empty())
                    .map(|word| quoted(&String::from_utf8_lossy(word)))
                    .collect();
                (!words.is_empty()).then(|| words.join(" "))
            });
            let started = boot
                .filter(|_| ticks > 0)
                .map(|boot| boot + Duration::from_millis(stat.start.saturating_mul(1000) / ticks as u64));
            let memory = fs::read_to_string(format!("/proc/{pid}/statm"))
                .ok()
                .and_then(|statm| statm.split_whitespace().nth(1)?.parse::<u64>().ok())
                .filter(|_| page > 0)
                .map(|pages| pages.saturating_mul(page as u64));
            found.push(Running { pid, name, command, started, memory });
        }
        Ok(found)
    }

    /// An argument as a command line writes it: in double quotes when it
    /// holds blank space, so the line still splits into the same arguments.
    fn quoted(word: &str) -> String {
        if word.contains(char::is_whitespace) {
            format!("\"{}\"", word.replace('\\', "\\\\").replace('"', "\\\""))
        } else {
            word.to_string()
        }
    }

    /// When the machine booted, as `/proc/stat` says: start times count
    /// from it.
    fn boot() -> Option<SystemTime> {
        let stat = fs::read_to_string("/proc/stat").ok()?;
        let seconds = stat.lines().find_map(|line| line.strip_prefix("btime "))?.trim().parse::<u64>().ok()?;
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::Running;
    use std::io;
    use std::time::{Duration, SystemTime};

    /// Each process by the image it runs; its command line is not read.
    pub(super) fn running() -> io::Result<Vec<Running>> {
        let mut found = Vec::new();
        for process in crate::darwin_proc::all()? {
            if process.zombie {
                continue;
            }
            let name = crate::darwin_proc::image(process.pid)
                .and_then(|image| image.file_name().map(|name| name.to_string_lossy().into_owned()))
                .unwrap_or_default();
            let started = Some(SystemTime::UNIX_EPOCH + Duration::from_micros(process.start));
            found.push(Running { pid: process.pid, name, command: None, started, memory: None });
        }
        Ok(found)
    }
}

#[cfg(windows)]
mod imp {
    use super::Running;
    use std::io;
    use std::time::{Duration, SystemTime};
    use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessCommandLineInformation};
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, UNICODE_STRING};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    pub(super) fn running() -> io::Result<Vec<Running>> {
        let mut found = Vec::new();
        // SAFETY: the snapshot is closed below, and the entry carries its size.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snapshot, &mut entry);
            while more != 0 {
                let pid = entry.th32ProcessID;
                if pid != 0 {
                    let length = entry.szExeFile.iter().position(|unit| *unit == 0).unwrap_or(entry.szExeFile.len());
                    let name = String::from_utf16_lossy(&entry.szExeFile[..length]);
                    let (command, started, memory) = details(pid);
                    found.push(Running { pid, name, command, started, memory });
                }
                more = Process32NextW(snapshot, &mut entry);
            }
            CloseHandle(snapshot);
        }
        Ok(found)
    }

    /// What this user may read of `pid`: its command line, when it started
    /// and its working set; nothing of one it may not open.
    fn details(pid: u32) -> (Option<String>, Option<SystemTime>, Option<u64>) {
        // SAFETY: a handle opened for querying only, closed below.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return (None, None, None);
        }
        let details = (command_line(process), started(process), memory(process));
        // SAFETY: the handle was opened above and is not used again.
        unsafe { CloseHandle(process) };
        details
    }

    fn command_line(process: HANDLE) -> Option<String> {
        let mut needed = 0u32;
        // SAFETY: a zero-length query only asks how much room the answer takes.
        unsafe {
            NtQueryInformationProcess(process, ProcessCommandLineInformation, std::ptr::null_mut(), 0, &mut needed)
        };
        if (needed as usize) < size_of::<UNICODE_STRING>() || needed > 1 << 20 {
            return None;
        }
        // Words of eight bytes keep the string's header at the front aligned.
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
        // SAFETY: the buffer holds `needed` bytes, and the answer is a
        // UNICODE_STRING whose characters follow it inside the buffer.
        unsafe {
            if NtQueryInformationProcess(
                process,
                ProcessCommandLineInformation,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            ) < 0
            {
                return None;
            }
            let text = &*(buffer.as_ptr() as *const UNICODE_STRING);
            if text.Buffer.is_null() || text.Length == 0 {
                return None;
            }
            let units = std::slice::from_raw_parts(text.Buffer, text.Length as usize / 2);
            Some(String::from_utf16_lossy(units))
        }
    }

    fn started(process: HANDLE) -> Option<SystemTime> {
        let mut times = [FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }; 4];
        let [created, exited, kernel, user] = &mut times;
        // SAFETY: four FILETIMEs to be written.
        if unsafe { GetProcessTimes(process, created, exited, kernel, user) } == 0 {
            return None;
        }
        // Hundreds of nanoseconds since 1601; the Unix epoch is 11644473600 s later.
        let since_1601 = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
        let since_epoch = since_1601.checked_sub(116_444_736_000_000_000)?;
        Some(SystemTime::UNIX_EPOCH + Duration::from_nanos(since_epoch.saturating_mul(100)))
    }

    fn memory(process: HANDLE) -> Option<u64> {
        // SAFETY: zeroed counters are valid, and their size is said.
        let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        let size = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        counters.cb = size;
        // SAFETY: the counters hold `size` bytes.
        (unsafe { K32GetProcessMemoryInfo(process, &mut counters, size) } != 0)
            .then_some(counters.WorkingSetSize as u64)
    }
}
