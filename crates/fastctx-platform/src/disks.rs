//! The computer's local fixed disks, and walking them without getting in
//! anyone's way: where a walk of every disk starts, and the thread it runs
//! on asking for no more of the processor and the disks than is left over.

use std::io;
use std::path::PathBuf;

/// The root of each local fixed disk, each volume once. On Windows, every
/// drive letter of a fixed volume; a letter that merely aliases a folder
/// (`subst`) is no volume of its own and is left out. On Linux, each mount
/// of a filesystem kept on a disk; pseudo, memory and network filesystems
/// are not disks.
pub fn fixed_disks() -> io::Result<Vec<PathBuf>> {
    imp::fixed_disks()
}

/// Puts the calling thread in the background: the lowest processor
/// priority, and on Windows the background mode that also lowers its disk
/// and memory priority; on Linux the idle disk class. It lasts until the
/// thread ends, so a thread made for one walk is the one to call it on.
pub fn lower_this_thread() -> io::Result<()> {
    imp::lower_this_thread()
}

#[cfg(windows)]
mod imp {
    use std::collections::BTreeSet;
    use std::ffi::OsString;
    use std::io;
    use std::os::windows::ffi::OsStringExt as _;
    use std::path::PathBuf;

    use windows_sys::Win32::Storage::FileSystem::{
        GetDriveTypeW, GetLogicalDriveStringsW, GetVolumeNameForVolumeMountPointW,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};

    /// `DRIVE_FIXED` of `GetDriveTypeW`.
    const DRIVE_FIXED: u32 = 3;

    pub fn fixed_disks() -> io::Result<Vec<PathBuf>> {
        // SAFETY: a size query with no buffer.
        let length = unsafe { GetLogicalDriveStringsW(0, std::ptr::null_mut()) };
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0u16; length as usize];
        // SAFETY: the buffer holds `length` units, as the system asked.
        let written = unsafe { GetLogicalDriveStringsW(length, buffer.as_mut_ptr()) };
        if written == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut volumes = BTreeSet::new();
        let mut disks = Vec::new();
        for root in buffer[..written as usize].split(|unit| *unit == 0).filter(|root| !root.is_empty()) {
            let mut wide: Vec<u16> = root.to_vec();
            wide.push(0);
            // SAFETY: a NUL-terminated root such as `C:\`.
            if unsafe { GetDriveTypeW(wide.as_ptr()) } != DRIVE_FIXED {
                continue;
            }
            // A volume GUID path is 49 units with its NUL.
            let mut volume = [0u16; 64];
            // SAFETY: the root is NUL-terminated and the buffer's length is
            // given. A `subst` letter is no mount point and fails here.
            if unsafe { GetVolumeNameForVolumeMountPointW(wide.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) }
                == 0
            {
                continue;
            }
            let end = volume.iter().position(|unit| *unit == 0).unwrap_or(volume.len());
            if volumes.insert(volume[..end].to_vec()) {
                disks.push(PathBuf::from(OsString::from_wide(root)));
            }
        }
        Ok(disks)
    }

    pub fn lower_this_thread() -> io::Result<()> {
        // SAFETY: the pseudo handle of the calling thread.
        if unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN as i32) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(unix)]
mod imp {
    use std::collections::BTreeSet;
    use std::io;
    use std::path::PathBuf;

    /// Filesystems kept on a disk; every other type is a pseudo, memory,
    /// image or network filesystem.
    const DISK_FILESYSTEMS: &[&str] = &[
        "ext2", "ext3", "ext4", "xfs", "btrfs", "zfs", "f2fs", "jfs", "reiserfs", "bcachefs", "vfat", "exfat", "ntfs",
        "ntfs3", "fuseblk", "hfsplus", "apfs",
    ];

    pub fn fixed_disks() -> io::Result<Vec<PathBuf>> {
        let mounts = std::fs::read_to_string("/proc/self/mounts")?;
        let mut seen = BTreeSet::new();
        let mut disks = Vec::new();
        for line in mounts.lines() {
            let mut fields = line.split_whitespace();
            let (Some(_source), Some(target), Some(kind)) = (fields.next(), fields.next(), fields.next()) else {
                continue;
            };
            if !DISK_FILESYSTEMS.contains(&kind) {
                continue;
            }
            let target = PathBuf::from(unescape(target));
            if seen.insert(target.clone()) {
                disks.push(target);
            }
        }
        Ok(disks)
    }

    /// `/proc/self/mounts` writes a space, tab, newline and backslash in a
    /// path as three octal digits after a backslash.
    fn unescape(field: &str) -> String {
        let bytes = field.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut at = 0;
        while at < bytes.len() {
            if bytes[at] == b'\\'
                && at + 3 < bytes.len()
                && bytes[at + 1..at + 4].iter().all(|digit| (b'0'..=b'7').contains(digit))
            {
                out.push((bytes[at + 1] - b'0') * 64 + (bytes[at + 2] - b'0') * 8 + (bytes[at + 3] - b'0'));
                at += 4;
            } else {
                out.push(bytes[at]);
                at += 1;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    pub fn lower_this_thread() -> io::Result<()> {
        // Linux keeps a niceness and a disk priority for each thread.
        #[cfg(target_os = "linux")]
        {
            // SAFETY: gettid has no arguments and cannot fail.
            let thread = unsafe { libc::syscall(libc::SYS_gettid) } as libc::id_t;
            // SAFETY: setpriority of one thread, by its id.
            if unsafe { libc::setpriority(libc::PRIO_PROCESS, thread, 19) } != 0 {
                return Err(io::Error::last_os_error());
            }
            /// `IOPRIO_WHO_PROCESS` and the idle class, shifted into place
            /// as `linux/ioprio.h` defines them.
            const WHO_PROCESS: libc::c_long = 1;
            const IDLE: libc::c_long = 3 << 13;
            // SAFETY: ioprio_set of one thread, by its id.
            if unsafe { libc::syscall(libc::SYS_ioprio_set, WHO_PROCESS, thread as libc::c_long, IDLE) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            // SAFETY: setpriority of this process.
            if unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 19) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}
