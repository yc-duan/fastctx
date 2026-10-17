//! Opening ordinary files without blocking on FIFOs, and comparing the
//! filesystem's identity and change indicators across a streaming read.

use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::path::Path;
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub size: u64,
    pub modified: SystemTime,
    identity: (u64, u64),
    changed: (i64, i64),
}

/// Opens for reading with concurrent writers allowed. Special files must
/// still be rejected from the returned handle's metadata by the caller.
pub fn open(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // A FIFO can replace an ordinary file between stat and open.
        options.custom_flags(libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        // Opening directories allows the caller to distinguish them from
        // access denial using the actual opened handle.
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS);
    }
    options.open(path)
}

impl Snapshot {
    /// Growth is meaningful only while the path still names the opened file.
    pub fn same_file(&self, other: &Self) -> bool {
        self.identity == other.identity
    }
    /// The snapshot as one line, for keeping beside what was read from the
    /// file: a replaced or rewritten file has another.
    pub fn fingerprint(&self) -> String {
        let modified = self.modified.duration_since(SystemTime::UNIX_EPOCH).map(|since| since.as_nanos()).unwrap_or(0);
        format!(
            "{}-{}-{}-{}-{}-{}",
            self.size, modified, self.identity.0, self.identity.1, self.changed.0, self.changed.1
        )
    }
}

pub fn snapshot(file: &File, metadata: &Metadata) -> io::Result<Snapshot> {
    let (identity, changed) = indicators(file, metadata)?;
    Ok(Snapshot { size: metadata.len(), modified: metadata.modified()?, identity, changed })
}

#[cfg(unix)]
fn indicators(_file: &File, metadata: &Metadata) -> io::Result<((u64, u64), (i64, i64))> {
    use std::os::unix::fs::MetadataExt as _;
    Ok(((metadata.dev(), metadata.ino()), (metadata.ctime(), metadata.ctime_nsec())))
}

#[cfg(windows)]
fn indicators(file: &File, _metadata: &Metadata) -> io::Result<((u64, u64), (i64, i64))> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FileBasicInfo, GetFileInformationByHandle,
        GetFileInformationByHandleEx,
    };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let mut basic: FILE_BASIC_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: file owns the handle and both outputs have their declared sizes.
    unsafe {
        if GetFileInformationByHandle(file.as_raw_handle(), &mut info) == 0
            || GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileBasicInfo,
                (&raw mut basic).cast(),
                std::mem::size_of::<FILE_BASIC_INFO>() as u32,
            ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((
        (u64::from(info.dwVolumeSerialNumber), (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)),
        (basic.ChangeTime, basic.CreationTime),
    ))
}
