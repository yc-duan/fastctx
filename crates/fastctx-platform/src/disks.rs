//! The computer's local fixed disks and where its filesystems are mounted,
//! and a thread that walks a folder without getting in anyone's way, asking
//! for no more of the processor and the disks than is left over.

use std::io;
use std::path::PathBuf;

/// The root of each local fixed disk, each volume once. On Windows, every
/// drive letter of a fixed volume; a letter that merely aliases a folder
/// (`subst`) is no volume of its own and is left out. On Linux, one mount
/// of each filesystem kept on a disk, the one showing the whole of it at
/// the shortest path; pseudo, memory and network filesystems are not disks.
pub fn fixed_disks() -> io::Result<Vec<PathBuf>> {
    imp::fixed_disks()
}

/// Every place a filesystem is mounted, which a walk of a folder does not
/// enter: on Linux the same filesystem can be mounted again inside itself
/// (a bind mount), and walking it there would walk it twice. On Windows
/// none: a folder a volume is mounted on is a reparse point, which a walk
/// never enters anyway.
pub fn mount_points() -> io::Result<Vec<PathBuf>> {
    imp::mount_points()
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

    pub fn mount_points() -> io::Result<Vec<PathBuf>> {
        Ok(Vec::new())
    }

    pub fn lower_this_thread() -> io::Result<()> {
        // SAFETY: the pseudo handle of the calling thread.
        if unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::collections::BTreeMap;
    use std::io;
    use std::path::PathBuf;

    /// Filesystems kept on a disk; every other type is a pseudo, memory,
    /// image or network filesystem.
    const DISK_FILESYSTEMS: &[&str] = &[
        "ext2", "ext3", "ext4", "xfs", "btrfs", "zfs", "f2fs", "jfs", "reiserfs", "bcachefs", "vfat", "exfat", "ntfs",
        "ntfs3", "fuseblk", "hfsplus", "apfs",
    ];

    use crate::mountinfo::{Mount, read as mounts};

    pub fn fixed_disks() -> io::Result<Vec<PathBuf>> {
        let mut by_device: BTreeMap<String, Vec<Mount>> = BTreeMap::new();
        for mount in mounts()? {
            if DISK_FILESYSTEMS.contains(&mount.kind.as_str()) {
                by_device.entry(mount.device.clone()).or_default().push(mount);
            }
        }
        // Of each filesystem, the mounts that together show all of it that
        // is mounted anywhere, each part once: the widest parts first, at
        // their shortest paths, and no part inside one already taken.
        let mut disks = Vec::new();
        for mut mounts in by_device.into_values() {
            mounts.sort_by_key(|mount| (mount.root.components().count(), mount.point.clone()));
            let mut taken: Vec<PathBuf> = Vec::new();
            for mount in mounts {
                let root = mount.root;
                if !taken.iter().any(|part| root.starts_with(part)) {
                    taken.push(root);
                    disks.push(mount.point);
                }
            }
        }
        disks.sort();
        Ok(disks)
    }

    pub fn mount_points() -> io::Result<Vec<PathBuf>> {
        Ok(mounts()?.into_iter().map(|mount| mount.point).collect())
    }

    pub fn lower_this_thread() -> io::Result<()> {
        // Linux keeps a niceness and a disk priority for each thread.
        // SAFETY: gettid has no arguments and cannot fail.
        let thread = unsafe { libc::syscall(libc::SYS_gettid) } as libc::id_t;
        // SAFETY: setpriority of one thread, by its id.
        if unsafe { libc::setpriority(libc::PRIO_PROCESS, thread, 19) } != 0 {
            return Err(io::Error::last_os_error());
        }
        /// `IOPRIO_WHO_PROCESS` and the idle class, shifted into place as
        /// `linux/ioprio.h` defines them.
        const WHO_PROCESS: libc::c_long = 1;
        const IDLE: libc::c_long = 3 << 13;
        // SAFETY: ioprio_set of one thread, by its id.
        if unsafe { libc::syscall(libc::SYS_ioprio_set, WHO_PROCESS, thread as libc::c_long, IDLE) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// On macOS the mounts are read with getmntinfo: a disk is a local volume a
/// person browses, the startup disk and those under `/Volumes`; the system's
/// own hidden volumes (its data, preboot and VM volumes) are not walked again.
#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::CStr;
    use std::io;
    use std::path::PathBuf;

    /// Filesystems kept on a disk.
    const DISK_FILESYSTEMS: &[&str] = &["apfs", "hfs", "exfat", "msdos", "ntfs", "ufs"];

    struct Mount {
        point: PathBuf,
        kind: String,
        flags: u32,
    }

    fn mounts() -> io::Result<Vec<Mount>> {
        let mut table: *mut libc::statfs = std::ptr::null_mut();
        // SAFETY: getmntinfo points `table` at its own buffer of `count`
        // entries, valid until the next call on this thread.
        let count = unsafe { libc::getmntinfo(&mut table, libc::MNT_NOWAIT) };
        if count <= 0 || table.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the buffer holds `count` initialized entries.
        let entries = unsafe { std::slice::from_raw_parts(table, count as usize) };
        Ok(entries
            .iter()
            .map(|entry| {
                // SAFETY: both names are NUL-terminated within their arrays.
                let (point, kind) = unsafe {
                    (CStr::from_ptr(entry.f_mntonname.as_ptr()), CStr::from_ptr(entry.f_fstypename.as_ptr()))
                };
                Mount {
                    point: PathBuf::from(point.to_string_lossy().into_owned()),
                    kind: kind.to_string_lossy().into_owned(),
                    flags: entry.f_flags,
                }
            })
            .collect())
    }

    pub fn fixed_disks() -> io::Result<Vec<PathBuf>> {
        let mut disks: Vec<PathBuf> = mounts()?
            .into_iter()
            .filter(|mount| {
                DISK_FILESYSTEMS.contains(&mount.kind.as_str())
                    && mount.flags & libc::MNT_LOCAL as u32 != 0
                    && mount.flags & libc::MNT_DONTBROWSE as u32 == 0
            })
            .map(|mount| mount.point)
            .collect();
        disks.sort();
        disks.dedup();
        Ok(disks)
    }

    pub fn mount_points() -> io::Result<Vec<PathBuf>> {
        Ok(mounts()?.into_iter().map(|mount| mount.point).collect())
    }

    pub fn lower_this_thread() -> io::Result<()> {
        // The background band lowers this thread's processor, disk and
        // network priority together.
        // SAFETY: setpriority of the calling thread.
        if unsafe { libc::setpriority(libc::PRIO_DARWIN_THREAD, 0, libc::PRIO_DARWIN_BG) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// Elsewhere neither the disks nor a thread's own background priority are
/// known to the World yet: a walk there fails, saying so, rather than walk
/// at the priority of the whole node.
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod imp {
    use std::io;
    use std::path::PathBuf;

    fn unsupported() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, "not on this operating system")
    }

    pub fn fixed_disks() -> io::Result<Vec<PathBuf>> {
        Err(unsupported())
    }

    pub fn mount_points() -> io::Result<Vec<PathBuf>> {
        Err(unsupported())
    }

    pub fn lower_this_thread() -> io::Result<()> {
        Err(unsupported())
    }
}
