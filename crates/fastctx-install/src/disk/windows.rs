use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr;
use std::time::SystemTime;

use windows_sys::Win32::Foundation::{ERROR_HANDLE_EOF, ERROR_INSUFFICIENT_BUFFER, GENERIC_READ, GENERIC_WRITE};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, GetKernelObjectSecurity, GetSecurityDescriptorControl,
    OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SE_DACL_AUTO_INHERIT_REQ, SE_DACL_AUTO_INHERITED,
    SE_DACL_PROTECTED, SetKernelObjectSecurity, SetSecurityDescriptorControl, UNPROTECTED_DACL_SECURITY_INFORMATION,
};
use windows_sys::Win32::Storage::FileSystem::*;

use super::Identity;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Attributes {
    flags: u32,
    created: i64,
    security: Vec<u32>,
    streams: Vec<(OsString, Vec<u8>)>,
}

impl Attributes {
    pub(crate) fn revision_bytes(&self) -> io::Result<Vec<u8>> {
        let mut attributes = self.clone();
        // ReplaceFile recomputes this derived marker even when every ACE and
        // protection bit is unchanged. It is not a change to access rights.
        let inherited = windows_sys::Win32::Security::SE_DACL_AUTO_INHERITED as u32;
        attributes.security[0] &= !(inherited << 16);
        serde_json::to_vec(&attributes).map_err(io::Error::other)
    }

    pub(crate) fn publication_bytes(&self) -> io::Result<Vec<u8>> {
        let mut attributes = self.clone();
        // ReplaceFile rebuilds inherited ACEs from the publication directory.
        // The complete displaced descriptor is checked separately; payload
        // identity/content must also remain recognizable after that OS change.
        attributes.security.clear();
        serde_json::to_vec(&attributes).map_err(io::Error::other)
    }

    pub fn has_named_data(&self) -> bool {
        !self.streams.is_empty()
    }

    pub fn read(file: &File, path: &Path) -> io::Result<Self> {
        let mut basic = FILE_BASIC_INFO::default();
        // SAFETY: the handle and output buffer are valid for the call.
        check(unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileBasicInfo,
                (&raw mut basic).cast(),
                size_of::<FILE_BASIC_INFO>() as u32,
            )
        })?;
        let info = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
        let mut needed = 0;
        // SAFETY: a null descriptor with zero length queries the required size.
        let result = unsafe { GetKernelObjectSecurity(file.as_raw_handle(), info, ptr::null_mut(), 0, &mut needed) };
        if result == 0 && io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
            return Err(io::Error::last_os_error());
        }
        let mut security = vec![0u32; (needed as usize).div_ceil(size_of::<u32>())];
        // SAFETY: the aligned buffer has at least the requested size.
        check(unsafe {
            GetKernelObjectSecurity(file.as_raw_handle(), info, security.as_mut_ptr().cast(), needed, &mut needed)
        })?;
        Ok(Self { flags: basic.FileAttributes, created: basic.CreationTime, security, streams: streams(file, path)? })
    }

    pub fn apply(&self, file: &File, path: &Path) -> io::Result<()> {
        self.apply_to(file, path, false)
    }

    pub fn apply_directory(&self, file: &File, path: &Path) -> io::Result<()> {
        self.apply_to(file, path, true)
    }

    pub fn apply_reparse_header(&self, file: &File) -> io::Result<()> {
        let basic = FILE_BASIC_INFO { CreationTime: self.created, FileAttributes: self.flags, ..Default::default() };
        // SAFETY: a live no-follow handle and initialized basic information.
        check(unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileBasicInfo,
                (&raw const basic).cast(),
                size_of::<FILE_BASIC_INFO>() as u32,
            )
        })
    }

    fn apply_to(&self, file: &File, path: &Path, directory: bool) -> io::Result<()> {
        // These flags require additional filesystem operations, rather than
        // SetFileInformationByHandle. Refuse to silently discard them.
        let supported = FILE_ATTRIBUTE_READONLY
            | FILE_ATTRIBUTE_HIDDEN
            | FILE_ATTRIBUTE_SYSTEM
            | FILE_ATTRIBUTE_ARCHIVE
            | FILE_ATTRIBUTE_NORMAL
            | FILE_ATTRIBUTE_TEMPORARY
            | FILE_ATTRIBUTE_OFFLINE
            | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED
            | if directory { FILE_ATTRIBUTE_DIRECTORY } else { 0 };
        if self.flags & !supported != 0 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("cannot preserve Windows file attributes {:#x}", self.flags),
            ));
        }
        for (name, bytes) in &self.streams {
            let mut stream = path.as_os_str().to_os_string();
            stream.push(name);
            fs::write(stream, bytes)?;
        }
        let basic = FILE_BASIC_INFO { CreationTime: self.created, FileAttributes: self.flags, ..Default::default() };
        // SAFETY: a correctly sized initialized FILE_BASIC_INFO and live handle.
        check(unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileBasicInfo,
                (&raw const basic).cast(),
                size_of::<FILE_BASIC_INFO>() as u32,
            )
        })
    }

    pub fn protect(&self, file: &File) -> io::Result<()> {
        let mut control = 0;
        let mut revision = 0;
        let mut security = self.security.clone();
        let descriptor = security.as_mut_ptr().cast();
        // SAFETY: read() captured a valid self-relative security descriptor.
        check(unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) })?;
        let protection = if control & SE_DACL_PROTECTED != 0 {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };
        // SetKernelObjectSecurity consumes AUTO_INHERIT_REQ when restoring
        // AUTO_INHERITED. Passing only the latter silently drops it; unlike
        // SetSecurityInfo this does not inherit the archive parent's ACEs or
        // rewrite existing children (verified on Windows 11, 2026-10-04).
        check(unsafe {
            SetSecurityDescriptorControl(
                descriptor,
                SE_DACL_AUTO_INHERIT_REQ,
                if control & SE_DACL_AUTO_INHERITED != 0 { SE_DACL_AUTO_INHERIT_REQ } else { 0 },
            )
        })?;
        // SAFETY: the descriptor is valid and the staging handle owns these rights.
        check(unsafe {
            SetKernelObjectSecurity(
                file.as_raw_handle(),
                OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | protection,
                descriptor,
            )
        })
    }

    pub fn writable(&self) -> bool {
        self.flags & FILE_ATTRIBUTE_READONLY == 0
    }
}

fn streams(file: &File, path: &Path) -> io::Result<Vec<(OsString, Vec<u8>)>> {
    let expected = identity(file)?;
    let mut buffer = vec![0u64; size_of::<FILE_STREAM_INFO>().div_ceil(8)];
    loop {
        // SAFETY: the buffer is aligned for FILE_STREAM_INFO and grows when
        // the kernel reports it cannot hold the complete stream list.
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileStreamInfo,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
            )
        } != 0
        {
            break;
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_HANDLE_EOF as i32) {
            return Ok(Vec::new());
        }
        if matches!(error.raw_os_error(), Some(code) if code == ERROR_INSUFFICIENT_BUFFER as i32 || code == windows_sys::Win32::Foundation::ERROR_MORE_DATA as i32)
        {
            buffer.resize(buffer.len() * 2, 0);
        } else {
            return Err(error);
        }
    }
    let mut offset = 0usize;
    let mut result = Vec::new();
    loop {
        let header = offset_of!(FILE_STREAM_INFO, StreamName);
        if offset + header > buffer.len() * 8 {
            return Err(io::Error::other("invalid stream information"));
        }
        // SAFETY: the header is within the aligned kernel buffer.
        let info = unsafe { &*(buffer.as_ptr().cast::<u8>().add(offset).cast::<FILE_STREAM_INFO>()) };
        let length = info.StreamNameLength as usize;
        if !length.is_multiple_of(2) || offset + header + length > buffer.len() * 8 {
            return Err(io::Error::other("invalid stream name length"));
        }
        // SAFETY: the validated name contains length / 2 UTF-16 units.
        let name = OsString::from_wide(unsafe {
            std::slice::from_raw_parts((&raw const info.StreamName).cast::<u16>(), length / 2)
        });
        if name != "::$DATA" {
            let mut stream = path.as_os_str().to_os_string();
            stream.push(&name);
            let mut reader = OpenOptions::new().read(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(stream)?;
            if identity(&reader)? != expected {
                return Err(io::Error::other("stream belongs to another filesystem object"));
            }
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes)?;
            result.push((name, bytes));
        }
        if info.NextEntryOffset == 0 {
            break;
        }
        if info.NextEntryOffset as usize <= header || info.NextEntryOffset % 8 != 0 {
            return Err(io::Error::other("invalid stream entry offset"));
        }
        offset += info.NextEntryOffset as usize;
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

pub(crate) fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(path)
}

pub(crate) fn open_directory(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS).open(path)
}

pub(crate) fn create(path: &Path, _mode: u32) -> io::Result<File> {
    OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE | READ_CONTROL | WRITE_DAC | WRITE_OWNER)
        .open(path)
}

pub(crate) fn identity(file: &File) -> io::Result<Identity> {
    let mut info = FILE_ID_INFO::default();
    // SAFETY: a live file handle and a correctly sized output buffer.
    check(unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&raw mut info).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    })?;
    Ok(Identity { volume: info.VolumeSerialNumber, file: u128::from_le_bytes(info.FileId.Identifier) })
}

pub(crate) fn is_link(metadata: &fs::Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

pub(crate) fn publish(file: &File, _stage: &Path, target: &Path, replace: bool) -> io::Result<()> {
    let name = wide(target);
    let total = offset_of!(FILE_RENAME_INFO, FileName) + name.len() * size_of::<u16>();
    let mut buffer = vec![0u64; total.div_ceil(size_of::<u64>())];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // FileRenameInfoEx uses 1 for REPLACE_IF_EXISTS and 2 for POSIX_SEMANTICS.
    // The Win32 layer still requires a terminating NUL despite FileNameLength.
    // SAFETY: the buffer is aligned and sized for the header and terminated name.
    unsafe {
        (*info).Anonymous.Flags = 2 | u32::from(replace);
        (*info).FileNameLength = u32::try_from((name.len() - 1) * size_of::<u16>()).map_err(io::Error::other)?;
        ptr::copy_nonoverlapping(name.as_ptr(), (&raw mut (*info).FileName).cast::<u16>(), name.len());
        check(SetFileInformationByHandle(
            file.as_raw_handle(),
            FileRenameInfoEx,
            info.cast(),
            u32::try_from(total).map_err(io::Error::other)?,
        ))
    }
}

pub(crate) fn stage_survives_publication(_replace: bool) -> bool {
    false
}

pub(crate) fn delete_staged(file: &File, _path: &Path) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    // SAFETY: the staging file was opened with DELETE access. Clearing the
    // name by handle cannot accidentally delete a different directory entry.
    check(unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfoEx,
            (&raw const info).cast(),
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    })
}

pub(crate) fn delete(file: &File, path: &Path) -> io::Result<()> {
    delete_with_flags(file, path, FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS)
}

/// Retires an owned name without changing attributes shared by hard links.
pub(crate) fn delete_owned(file: &File, path: &Path) -> io::Result<()> {
    delete_with_flags(
        file,
        path,
        FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    )
}

fn delete_with_flags(file: &File, path: &Path, flags: u32) -> io::Result<()> {
    let deleting = OpenOptions::new()
        .read(true)
        .access_mode(DELETE | GENERIC_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if identity(&deleting)? != identity(file)? {
        return Err(io::Error::other("file identity changed before deletion"));
    }
    let info = FILE_DISPOSITION_INFO_EX { Flags: flags };
    // SAFETY: deletion is by this verified file's handle, with DELETE access.
    check(unsafe {
        SetFileInformationByHandle(
            deleting.as_raw_handle(),
            FileDispositionInfoEx,
            (&raw const info).cast(),
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    })
}

pub(crate) fn restore_modified(file: &File, modified: SystemTime) -> io::Result<()> {
    file.set_modified(modified)
}

pub(crate) fn sync_parent(path: &Path) -> io::Result<()> {
    sync_directory(path.parent().expect("absolute path"))
}

pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    let directory = OpenOptions::new()
        .write(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let mut filesystem = [0u16; 32];
    // MS-FSA 2.1.5.7 requires directory metadata on stable storage. Its
    // Windows product note 80 limits that behavior to NTFS: other drivers
    // can return success without flushing anything. Refuse that false promise.
    // SAFETY: a live directory handle and a correctly sized name buffer.
    check(unsafe {
        GetVolumeInformationByHandleW(
            directory.as_raw_handle(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            filesystem.as_mut_ptr(),
            filesystem.len() as u32,
        )
    })?;
    if filesystem[..5] != [b'N' as u16, b'T' as u16, b'F' as u16, b'S' as u16, 0] {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "durable directory publication requires NTFS"));
    }
    directory.sync_all()
}

/// Keeps the actual displaced object, including writes through old handles.
/// ReplaceFile can partially complete on error; callers must inspect all names.
pub(crate) fn replace_preserving(source: &Path, target: &Path, displaced: &Path) -> io::Result<()> {
    let (source, target, displaced) = (wide(source), wide(target), wide(displaced));
    // SAFETY: all three names are terminated, distinct paths on the same volume.
    check(unsafe { ReplaceFileW(target.as_ptr(), source.as_ptr(), displaced.as_ptr(), 0, ptr::null(), ptr::null()) })
}

/// Moves the object opened at the source to a new name without replacing one.
pub(crate) fn move_preserving(source: &Path, destination: &Path) -> io::Result<()> {
    let file = OpenOptions::new()
        .read(true)
        .access_mode(DELETE | GENERIC_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(source)?;
    publish(&file, source, destination, false)
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

fn check(result: i32) -> io::Result<()> {
    if result == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}
