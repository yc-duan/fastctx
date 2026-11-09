//! Opening ordinary files without blocking on FIFOs, and comparing the
//! filesystem's identity and change indicators across a streaming read.

use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::path::Path;
use std::time::SystemTime;

pub use fastctx_types::file_identity::{FileChangeStamp, FileIdentity};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub size: u64,
    pub modified: SystemTime,
    identity: (u64, u64),
    metadata_indicators: (i64, i64),
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
    /// Stable coordinates of the opened file, independent of path and length.
    /// They are meaningful only on this host and do not establish ownership.
    pub fn identity(&self) -> FileIdentity {
        use fastctx_types::sync::cursor::Count;
        if cfg!(windows) {
            FileIdentity::Windows { volume: Count(self.identity.0), index: Count(self.identity.1) }
        } else {
            FileIdentity::Unix { device: Count(self.identity.0), inode: Count(self.identity.1) }
        }
    }

    /// Serializable change observations, deliberately separate from identity.
    pub fn change_stamp(&self) -> io::Result<FileChangeStamp> {
        Ok(FileChangeStamp {
            size: fastctx_types::sync::cursor::ByteCount(self.size),
            modified: jiff::Timestamp::try_from(self.modified).map_err(io::Error::other)?,
            indicators: [self.metadata_indicators.0.to_string(), self.metadata_indicators.1.to_string()],
        })
    }

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
            self.size,
            modified,
            self.identity.0,
            self.identity.1,
            self.metadata_indicators.0,
            self.metadata_indicators.1
        )
    }
}

pub fn snapshot(file: &File, metadata: &Metadata) -> io::Result<Snapshot> {
    let (identity, metadata_indicators) = indicators(file, metadata)?;
    Ok(Snapshot { size: metadata.len(), modified: metadata.modified()?, identity, metadata_indicators })
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
