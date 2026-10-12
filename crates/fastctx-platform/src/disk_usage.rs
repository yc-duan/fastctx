//! Apparent disk usage, with the same link semantics as `du -sb`.

use std::collections::HashSet;
use std::io;
use std::path::Path;

/// Visits directories and files, counting each hard-linked file once and
/// never following symbolic links or Windows junctions.
pub fn visit(path: &Path, mut visitor: impl FnMut(&Path, &std::fs::Metadata, u64)) -> io::Result<()> {
    walk(path, &mut HashSet::new(), &mut visitor)
}

pub fn visit_many<'a>(
    paths: impl IntoIterator<Item = &'a Path>,
    mut visitor: impl FnMut(&Path, &std::fs::Metadata, u64),
) -> io::Result<()> {
    let mut seen = HashSet::new();
    for path in paths {
        walk(path, &mut seen, &mut visitor)?;
    }
    Ok(())
}

fn walk(
    path: &Path,
    seen: &mut HashSet<(u64, u64)>,
    visitor: &mut impl FnMut(&Path, &std::fs::Metadata, u64),
) -> io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let link = crate::owned_directory::is_link(&metadata);
    let bytes = if metadata.is_file() && !link && !seen.insert(identity(path, &metadata)?) {
        0
    } else {
        apparent(path, &metadata, link)?
    };
    visitor(path, &metadata, bytes);
    if metadata.is_dir() && !link {
        let mut entries =
            std::fs::read_dir(path)?.map(|entry| entry.map(|entry| entry.path())).collect::<io::Result<Vec<_>>>()?;
        entries.sort();
        for path in entries {
            walk(&path, seen, visitor)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn apparent(_path: &Path, metadata: &std::fs::Metadata, _link: bool) -> io::Result<u64> {
    Ok(metadata.len())
}

#[cfg(windows)]
fn apparent(path: &Path, metadata: &std::fs::Metadata, link: bool) -> io::Result<u64> {
    if link {
        if reparse_tag(path)? == 0x80000023 {
            // IO_REPARSE_TAG_AF_UNIX is a socket, not a filesystem alias.
            return Ok(metadata.len());
        }
        // MSYS du measures the UTF-8 link target without the NT path prefix.
        let target = std::fs::read_link(path)?;
        let target = target.to_string_lossy();
        let bytes = if let Some(unc) = target.strip_prefix("\\\\?\\UNC\\") {
            unc.len() + 2
        } else {
            target.strip_prefix("\\\\?\\").unwrap_or(&target).len()
        };
        return Ok(bytes as u64);
    }
    Ok(if metadata.is_dir() { 0 } else { metadata.len() })
}

#[cfg(windows)]
fn reparse_tag(path: &Path) -> io::Result<u32> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{FindClose, FindFirstFileW, WIN32_FIND_DATAW};
    let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut information: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    // SAFETY: the name and output buffer live through this synchronous call.
    let handle = unsafe { FindFirstFileW(name.as_ptr(), &mut information) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        FindClose(handle);
    }
    Ok(information.dwReserved0)
}

#[cfg(unix)]
fn identity(_path: &Path, metadata: &std::fs::Metadata) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn identity(path: &Path, _metadata: &std::fs::Metadata) -> io::Result<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
    let file = std::fs::File::open(path)?;
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: an owned file handle and a valid output buffer.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((
        u64::from(information.dwVolumeSerialNumber),
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    ))
}
