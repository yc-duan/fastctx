//! Processes as Linux shows them under `/proc`.

use std::fs;
use std::io;
use std::sync::OnceLock;

/// The parts of `/proc/<pid>/stat` the World reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    pub pid: u32,
    pub ppid: u32,
    /// One letter: `R`, `S`, `D`, `Z` and so on.
    pub state: u8,
    /// Clock ticks since boot at which the process started.
    pub start: u64,
    /// The kernel's per-process flags.
    pub flags: u64,
}

/// Set from the moment a process starts to exit: its files are closing,
/// and it becomes a zombie a moment later.
const PF_EXITING: u64 = 0x4;

impl Stat {
    /// A process that has exited, whether or not it has been reaped.
    pub fn has_exited(&self) -> bool {
        self.state == b'Z' || self.state == b'X'
    }

    /// A process that has exited or is in the middle of exiting.
    pub fn is_ending(&self) -> bool {
        self.has_exited() || self.flags & PF_EXITING != 0
    }
}

/// The process with `pid`, or `None` if there is none.
pub fn stat(pid: u32) -> io::Result<Option<Stat>> {
    let text = match fs::read(format!("/proc/{pid}/stat")) {
        Ok(text) => text,
        // ESRCH: the process went away while the file was being read.
        Err(error) if error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(libc::ESRCH) => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    parse(pid, &text).map(Some)
}

/// Every process now. One that exits while the list is being read is left
/// out.
pub fn all() -> io::Result<Vec<Stat>> {
    let mut processes = Vec::new();
    for entry in fs::read_dir("/proc")? {
        let Ok(pid) = entry?.file_name().to_string_lossy().parse::<u32>() else { continue };
        if let Some(stat) = stat(pid)? {
            processes.push(stat);
        }
    }
    Ok(processes)
}

fn parse(pid: u32, text: &[u8]) -> io::Result<Stat> {
    let malformed = || io::Error::other(format!("malformed /proc/{pid}/stat"));
    // The command name may contain anything, parentheses included; the
    // fields after it start after its last closing parenthesis.
    let close = text.iter().rposition(|&b| b == b')').ok_or_else(malformed)?;
    let fields: Vec<&[u8]> = text.get(close + 2..).ok_or_else(malformed)?.split(|&b| b == b' ').collect();
    // `fields` starts at field 3, the state; field 4 is the parent, field
    // 9 the flags, field 22 the start time.
    let number = |index: usize| -> io::Result<u64> {
        let field = fields.get(index).ok_or_else(malformed)?;
        std::str::from_utf8(field).ok().and_then(|text| text.parse().ok()).ok_or_else(malformed)
    };
    Ok(Stat {
        pid,
        ppid: number(1)? as u32,
        state: *fields.first().and_then(|state| state.first()).ok_or_else(malformed)?,
        start: number(19)?,
        flags: number(6)?,
    })
}

/// This boot's random identifier; start times count from the boot.
pub fn boot_id() -> io::Result<&'static str> {
    static BOOT_ID: OnceLock<String> = OnceLock::new();
    if let Some(id) = BOOT_ID.get() {
        return Ok(id);
    }
    let id = fs::read_to_string("/proc/sys/kernel/random/boot_id")?.trim().to_string();
    Ok(BOOT_ID.get_or_init(|| id))
}
