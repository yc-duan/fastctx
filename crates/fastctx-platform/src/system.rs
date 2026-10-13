//! What the operating system says of the machine as a whole: when it
//! started, its processors and how long they have been busy, its memory,
//! the space on its disks, and what its network cards have moved. Each is
//! read on its own, so what one system does not tell leaves the rest.
//!
//! Counters only grow; how busy the processors are, or how fast a card
//! moves bytes, is the difference between two readings, which is the
//! caller's to take.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

/// How long the machine has been running.
pub fn uptime() -> io::Result<Duration> {
    imp::uptime()
}

/// The processor's name, such as `AMD Ryzen 9 7950X 16-Core Processor`.
pub fn cpu_model() -> io::Result<String> {
    imp::cpu_model()
}

/// Logical processors.
pub fn threads() -> u32 {
    imp::threads()
}

/// How long all processors together have been idle, and running at all,
/// in the system's own units since it started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuTimes {
    pub idle: u64,
    pub total: u64,
}

impl CpuTimes {
    /// How busy the processors were from `before` to this, 0 to 1.
    pub fn busy_since(&self, before: &CpuTimes) -> Option<f32> {
        let total = self.total.checked_sub(before.total)?;
        let idle = self.idle.checked_sub(before.idle)?;
        (total > 0).then(|| (1.0 - idle as f64 / total as f64).clamp(0.0, 1.0) as f32)
    }
}

pub fn cpu_times() -> io::Result<CpuTimes> {
    imp::cpu_times()
}

/// The load averages over 1, 5 and 15 minutes, where the system keeps them.
pub fn load() -> Option<[f32; 3]> {
    imp::load()
}

/// Bytes. On Windows, `swap_*` is the commit limit and the commit charge,
/// what Windows itself shows of memory set aside on disk; elsewhere the
/// swap space and what of it is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryUse {
    pub total: u64,
    pub used: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

pub fn memory() -> io::Result<MemoryUse> {
    imp::memory()
}

/// One disk, at the root `fixed_disks` gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskUse {
    pub at: PathBuf,
    pub label: Option<String>,
    pub fs: String,
    pub total: u64,
    /// Free to this user.
    pub free: u64,
}

/// The space on each local fixed disk (`crate::disks::fixed_disks`); one
/// that cannot be read is left out.
pub fn disks() -> io::Result<Vec<DiskUse>> {
    Ok(crate::disks::fixed_disks()?.into_iter().filter_map(|root| imp::disk(root).ok()).collect())
}

/// What a network card has moved since it came up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Traffic {
    /// The system's index of the interface.
    pub index: u32,
    pub name: String,
    pub up: bool,
    /// Its link speed, in bits a second, where the system knows it.
    pub speed: Option<u64>,
    pub received: u64,
    pub sent: u64,
    /// Whether it is a card of the machine's own, not a virtual one.
    pub hardware: bool,
}

/// Every network card but the loopback and the filters stacked on cards.
pub fn traffic() -> io::Result<Vec<Traffic>> {
    imp::traffic()
}

/// The system as people call it, such as `Windows 11 Pro 24H2 (26200.6584)`
/// or `Ubuntu 24.04.1 LTS`.
pub fn system_name() -> io::Result<String> {
    imp::system_name()
}

/// The kernel's version, where the system's name does not say it.
pub fn kernel() -> Option<String> {
    imp::kernel()
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsString;
    use std::io;
    use std::os::windows::ffi::OsStringExt as _;
    use std::path::PathBuf;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetIfTable2, IF_TYPE_SOFTWARE_LOOPBACK, MIB_IF_TABLE2,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetVolumeInformationW};
    use windows_sys::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::{ALL_PROCESSOR_GROUPS, GetActiveProcessorCount, GetSystemTimes};

    use super::{CpuTimes, DiskUse, MemoryUse, Traffic};
    use crate::registry::{Hive, dword_value, string_value};

    pub fn uptime() -> io::Result<Duration> {
        // SAFETY: no arguments.
        Ok(Duration::from_millis(unsafe { GetTickCount64() }))
    }

    pub fn cpu_model() -> io::Result<String> {
        let name =
            string_value(Hive::LocalMachine, r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString")?;
        Ok(name.map(|name| name.to_string_lossy().trim().to_string()).unwrap_or_default())
    }

    pub fn threads() -> u32 {
        // SAFETY: no pointers.
        let count = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
        if count == 0 { std::thread::available_parallelism().map_or(1, |n| n.get() as u32) } else { count }
    }

    fn ticks(time: FILETIME) -> u64 {
        (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
    }

    pub fn cpu_times() -> io::Result<CpuTimes> {
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        // SAFETY: three times to fill.
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // The kernel's time includes the idle time.
        Ok(CpuTimes { idle: ticks(idle), total: ticks(kernel) + ticks(user) })
    }

    pub fn load() -> Option<[f32; 3]> {
        None
    }

    pub fn memory() -> io::Result<MemoryUse> {
        // SAFETY: a plain structure the call fills, its length set first.
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = std::mem::size_of_val(&status) as u32;
        // SAFETY: as above.
        if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(MemoryUse {
            total: status.ullTotalPhys,
            used: status.ullTotalPhys.saturating_sub(status.ullAvailPhys),
            swap_total: status.ullTotalPageFile,
            swap_used: status.ullTotalPageFile.saturating_sub(status.ullAvailPageFile),
        })
    }

    fn wide_text(units: &[u16]) -> String {
        let end = units.iter().position(|unit| *unit == 0).unwrap_or(units.len());
        OsString::from_wide(&units[..end]).to_string_lossy().into_owned()
    }

    pub fn disk(root: PathBuf) -> io::Result<DiskUse> {
        let wide: Vec<u16> = root.as_os_str().encode_wide_with_nul();
        let (mut free, mut total, mut all_free) = (0u64, 0u64, 0u64);
        // SAFETY: a NUL-terminated root; three numbers to fill.
        if unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut all_free) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut label = [0u16; 261];
        let mut fs = [0u16; 261];
        // SAFETY: the buffers' lengths are given; the rest is not asked for.
        let named = unsafe {
            GetVolumeInformationW(
                wide.as_ptr(),
                label.as_mut_ptr(),
                label.len() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        } != 0;
        let label = if named { wide_text(&label) } else { String::new() };
        Ok(DiskUse {
            at: root,
            label: (!label.is_empty()).then_some(label),
            fs: if named { wide_text(&fs) } else { String::new() },
            total,
            free,
        })
    }

    trait WideWithNul {
        fn encode_wide_with_nul(&self) -> Vec<u16>;
    }

    impl WideWithNul for std::ffi::OsStr {
        fn encode_wide_with_nul(&self) -> Vec<u16> {
            use std::os::windows::ffi::OsStrExt as _;
            self.encode_wide().chain([0]).collect()
        }
    }

    /// The flags `MIB_IF_ROW2` packs in a byte: its first bit marks a card
    /// of the machine's own, the second a filter stacked on one.
    const HARDWARE: u8 = 1;
    const FILTER: u8 = 2;

    pub fn traffic() -> io::Result<Vec<Traffic>> {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        // SAFETY: the system allocates the table; it is freed below.
        let code = unsafe { GetIfTable2(&mut table) };
        if code != 0 {
            return Err(io::Error::from_raw_os_error(code as i32));
        }
        struct Freed(*mut MIB_IF_TABLE2);
        impl Drop for Freed {
            fn drop(&mut self) {
                // SAFETY: the table GetIfTable2 made.
                unsafe { FreeMibTable(self.0.cast()) };
            }
        }
        let freed = Freed(table);
        // SAFETY: the table holds `NumEntries` rows from its first.
        let rows = unsafe { std::slice::from_raw_parts((*freed.0).Table.as_ptr(), (*freed.0).NumEntries as usize) };
        Ok(rows
            .iter()
            .filter(|row| {
                row.Type != IF_TYPE_SOFTWARE_LOOPBACK && row.InterfaceAndOperStatusFlags._bitfield & FILTER == 0
            })
            .map(|row| {
                let speed = row.ReceiveLinkSpeed.max(row.TransmitLinkSpeed);
                Traffic {
                    index: row.InterfaceIndex,
                    name: wide_text(&row.Alias),
                    up: row.OperStatus == IfOperStatusUp,
                    // An unknown speed is all ones.
                    speed: (speed != 0 && speed != u64::MAX).then_some(speed),
                    received: row.InOctets,
                    sent: row.OutOctets,
                    hardware: row.InterfaceAndOperStatusFlags._bitfield & HARDWARE != 0,
                }
            })
            .collect())
    }

    pub fn system_name() -> io::Result<String> {
        let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
        let text = |value: &str| -> io::Result<String> {
            Ok(string_value(Hive::LocalMachine, key, value)?
                .map(|text| text.to_string_lossy().trim().to_string())
                .unwrap_or_default())
        };
        let mut product = text("ProductName")?;
        let build = text("CurrentBuild")?;
        // Windows 11 still names itself Windows 10 here; its builds start at
        // 22000.
        if build.parse::<u32>().is_ok_and(|build| build >= 22000) {
            product = product.replacen("Windows 10", "Windows 11", 1);
        }
        let display = text("DisplayVersion")?;
        let revision = dword_value(Hive::LocalMachine, key, "UBR")?;
        let build = match revision {
            Some(revision) if !build.is_empty() => format!("{build}.{revision}"),
            _ => build,
        };
        let mut name = if product.is_empty() { "Windows".to_string() } else { product };
        if !display.is_empty() {
            name.push(' ');
            name.push_str(&display);
        }
        if !build.is_empty() {
            name.push_str(&format!(" ({build})"));
        }
        Ok(name)
    }

    pub fn kernel() -> Option<String> {
        None
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::io;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use super::{CpuTimes, DiskUse, MemoryUse, Traffic};

    fn invalid(what: &str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, format!("{what} was not understood"))
    }

    pub fn uptime() -> io::Result<Duration> {
        let text = std::fs::read_to_string("/proc/uptime")?;
        let seconds: f64 = text
            .split_whitespace()
            .next()
            .and_then(|first| first.parse().ok())
            .ok_or_else(|| invalid("/proc/uptime"))?;
        Ok(Duration::from_secs_f64(seconds))
    }

    pub fn cpu_model() -> io::Result<String> {
        let text = std::fs::read_to_string("/proc/cpuinfo")?;
        // x86 says `model name`; ARM kernels say `Model` or `Hardware`, or
        // nothing.
        for key in ["model name", "Model", "Hardware", "cpu model"] {
            if let Some(value) = text.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                (name.trim() == key).then(|| value.trim().to_string())
            }) {
                return Ok(value);
            }
        }
        Ok(String::new())
    }

    pub fn threads() -> u32 {
        // SAFETY: sysconf has no pointers.
        let online = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
        if online > 0 { online as u32 } else { std::thread::available_parallelism().map_or(1, |n| n.get() as u32) }
    }

    pub fn cpu_times() -> io::Result<CpuTimes> {
        let text = std::fs::read_to_string("/proc/stat")?;
        let line = text.lines().find(|line| line.starts_with("cpu ")).ok_or_else(|| invalid("/proc/stat"))?;
        // user nice system idle iowait irq softirq steal; guest time is in
        // user already.
        let fields: Vec<u64> =
            line.split_whitespace().skip(1).take(8).map(|field| field.parse().unwrap_or(0)).collect();
        if fields.len() < 4 {
            return Err(invalid("/proc/stat"));
        }
        let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
        Ok(CpuTimes { idle, total: fields.iter().sum() })
    }

    pub fn load() -> Option<[f32; 3]> {
        let text = std::fs::read_to_string("/proc/loadavg").ok()?;
        let mut fields = text.split_whitespace().map(|field| field.parse::<f32>().ok());
        Some([fields.next()??, fields.next()??, fields.next()??])
    }

    pub fn memory() -> io::Result<MemoryUse> {
        let text = std::fs::read_to_string("/proc/meminfo")?;
        let field = |name: &str| -> u64 {
            text.lines()
                .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
                .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
                .unwrap_or(0)
                * 1024
        };
        let total = field("MemTotal");
        let available = field("MemAvailable");
        let swap_total = field("SwapTotal");
        Ok(MemoryUse {
            total,
            used: total.saturating_sub(available),
            swap_total,
            swap_used: swap_total.saturating_sub(field("SwapFree")),
        })
    }

    /// The type of the filesystem mounted last at `point`.
    fn kind_at(point: &Path) -> String {
        let Ok(text) = std::fs::read_to_string("/proc/self/mountinfo") else { return String::new() };
        let wanted = point.to_string_lossy();
        text.lines()
            .filter_map(|line| {
                let fields: Vec<&str> = line.split(' ').collect();
                let separator = fields.iter().position(|field| *field == "-")?;
                (fields.get(4)? == &wanted.as_ref())
                    .then(|| fields.get(separator + 1).map(|kind| (*kind).to_string()))?
            })
            .last()
            .unwrap_or_default()
    }

    pub fn disk(root: PathBuf) -> io::Result<DiskUse> {
        use std::os::unix::ffi::OsStrExt as _;
        let path = std::ffi::CString::new(root.as_os_str().as_bytes()).map_err(io::Error::other)?;
        // SAFETY: a plain structure the call fills.
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: a NUL-terminated path and the structure.
        if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let fragment = stat.f_frsize as u64;
        Ok(DiskUse {
            fs: kind_at(&root),
            at: root,
            label: None,
            total: stat.f_blocks as u64 * fragment,
            free: stat.f_bavail as u64 * fragment,
        })
    }

    pub fn traffic() -> io::Result<Vec<Traffic>> {
        let mut cards = Vec::new();
        for entry in std::fs::read_dir("/sys/class/net")? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "lo" {
                continue;
            }
            let path = entry.path();
            let read = |file: &str| std::fs::read_to_string(path.join(file)).map(|text| text.trim().to_string());
            let number = |file: &str| read(file).ok().and_then(|text| text.parse::<u64>().ok());
            // The speed is in megabits; a card that is down or virtual has
            // none to say, or -1.
            let speed = read("speed").ok().and_then(|text| text.parse::<i64>().ok()).filter(|mbits| *mbits > 0);
            cards.push(Traffic {
                index: number("ifindex").unwrap_or(0) as u32,
                up: read("operstate").is_ok_and(|state| state == "up"),
                speed: speed.map(|mbits| mbits as u64 * 1_000_000),
                received: number("statistics/rx_bytes").unwrap_or(0),
                sent: number("statistics/tx_bytes").unwrap_or(0),
                hardware: path.join("device").exists(),
                name,
            });
        }
        cards.sort_by_key(|card| card.index);
        Ok(cards)
    }

    pub fn system_name() -> io::Result<String> {
        let text =
            std::fs::read_to_string("/etc/os-release").or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))?;
        let field = |key: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
                .map(|value| value.trim().trim_matches('"').to_string())
        };
        Ok(field("PRETTY_NAME").or_else(|| field("NAME")).unwrap_or_else(|| "Linux".to_string()))
    }

    pub fn kernel() -> Option<String> {
        std::fs::read_to_string("/proc/sys/kernel/osrelease").ok().map(|text| format!("Linux {}", text.trim()))
    }
}

/// Elsewhere the World does not read the machine yet: each reading says so.
#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use std::io;
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{CpuTimes, DiskUse, MemoryUse, Traffic};

    fn unsupported() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, "not read on this operating system yet")
    }

    pub fn uptime() -> io::Result<Duration> {
        Err(unsupported())
    }

    pub fn cpu_model() -> io::Result<String> {
        Err(unsupported())
    }

    pub fn threads() -> u32 {
        std::thread::available_parallelism().map_or(1, |n| n.get() as u32)
    }

    pub fn cpu_times() -> io::Result<CpuTimes> {
        Err(unsupported())
    }

    pub fn load() -> Option<[f32; 3]> {
        None
    }

    pub fn memory() -> io::Result<MemoryUse> {
        Err(unsupported())
    }

    pub fn disk(_root: PathBuf) -> io::Result<DiskUse> {
        Err(unsupported())
    }

    pub fn traffic() -> io::Result<Vec<Traffic>> {
        Err(unsupported())
    }

    pub fn system_name() -> io::Result<String> {
        Err(unsupported())
    }

    pub fn kernel() -> Option<String> {
        None
    }
}
