//! Single-component NT opens and renames relative to a held directory handle.
use super::{FileIdentity, Name, invalid};
use std::{
    ffi::{OsStr, c_void},
    fs::File,
    io,
    mem::{offset_of, size_of},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
    ptr,
};
use windows_sys::Win32::{
    Foundation::{HANDLE, UNICODE_STRING},
    Storage::FileSystem::*,
    System::IO::IO_STATUS_BLOCK,
};

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: HANDLE,
    name: *mut UNICODE_STRING,
    attributes: u32,
    security: *mut c_void,
    qos: *mut c_void,
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtCreateFile(
        handle: *mut HANDLE,
        access: u32,
        object: *const ObjectAttributes,
        status: *mut IO_STATUS_BLOCK,
        allocation: *const i64,
        attributes: u32,
        share: u32,
        disposition: u32,
        options: u32,
        ea: *const c_void,
        ea_length: u32,
    ) -> i32;
    fn NtSetInformationFile(
        handle: HANDLE,
        status: *mut IO_STATUS_BLOCK,
        information: *const c_void,
        length: u32,
        class: u32,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}
const SYNCHRONIZE: u32 = 0x00100000;
const READ_CONTROL: u32 = 0x00020000;
const FILE_OPEN: u32 = 1;
const FILE_CREATE: u32 = 2;
const OPEN_REPARSE: u32 = 0x00200000;
const SYNC_IO: u32 = 0x20;
const WRITE_THROUGH: u32 = 2;

pub(crate) fn status(code: i32) -> io::Result<()> {
    if code >= 0 {
        Ok(())
    } else {
        // SAFETY: converts the status value without dereferencing caller memory.
        Err(io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(code) } as i32))
    }
}
fn open(parent: HANDLE, name: &OsStr, access: u32, create: bool, directory: bool) -> io::Result<File> {
    let mut wide: Vec<u16> = name.encode_wide().collect();
    let length = u16::try_from(wide.len() * 2).map_err(|_| invalid("native name too long"))?;
    let mut unicode = UNICODE_STRING { Length: length, MaximumLength: length, Buffer: wide.as_mut_ptr() };
    let descriptor = if create { Some(crate::windows_user::OwnerOnly::new(directory)?) } else { None };
    let attrs = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root: parent,
        name: &mut unicode,
        attributes: 0x40 | 0x1000,
        security: descriptor.as_ref().map_or(ptr::null_mut(), |v| v.descriptor()),
        qos: ptr::null_mut(),
    };
    let mut ios = IO_STATUS_BLOCK::default();
    let mut handle = ptr::null_mut();
    // Opening is type-neutral with no-follow; creation is exclusive, so a
    // preexisting reparse point can never be used as a creation target.
    let options = SYNC_IO
        | if create {
            WRITE_THROUGH | if directory { 1 } else { 0x40 }
        } else {
            OPEN_REPARSE | if access & (FILE_WRITE_DATA | FILE_WRITE_ATTRIBUTES) != 0 { WRITE_THROUGH } else { 0 }
        };
    // SAFETY: structures/buffers remain live, handles are synchronous, and each
    // successful returned handle is owned once by File.
    status(unsafe {
        NtCreateFile(
            &mut handle,
            access | SYNCHRONIZE,
            &attrs,
            &mut ios,
            ptr::null(),
            if directory { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_NORMAL },
            if directory {
                FILE_SHARE_READ | FILE_SHARE_WRITE
            } else {
                FILE_SHARE_READ | FILE_SHARE_DELETE | if access & FILE_WRITE_DATA == 0 { FILE_SHARE_WRITE } else { 0 }
            },
            if create { FILE_CREATE } else { FILE_OPEN },
            options,
            ptr::null(),
            0,
        )
    })?;
    // SAFETY: NtCreateFile returned an owned valid handle.
    Ok(unsafe { File::from_raw_handle(handle) })
}
pub(super) fn absolute_components(path: &Path) -> io::Result<(PathBuf, Vec<Name>)> {
    let mut components = path.components();
    let drive = match components.next() {
        Some(Component::Prefix(p)) => match p.kind() {
            Prefix::Disk(v) | Prefix::VerbatimDisk(v) => v,
            _ => return Err(invalid("managed roots require a local drive")),
        },
        _ => return Err(invalid("managed roots require an absolute local drive")),
    };
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(invalid("drive-relative root is not allowed"));
    }
    let mut names = Vec::new();
    for component in components {
        let Component::Normal(value) = component else {
            return Err(invalid("root path must be normalized"));
        };
        names.push(Name::new(value)?);
    }
    Ok((PathBuf::from(format!("{}:\\", drive as char)), names))
}
pub(super) fn open_anchor(path: &Path) -> io::Result<File> {
    let name = std::ffi::OsString::from(format!("\\??\\{}", path.display()));
    let file = open(ptr::null_mut(), &name, FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL, false, true)?;
    plain_directory(&file)?;
    Ok(file)
}
fn info(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut value = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: valid handle and writable structure.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut value) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(value)
}
fn plain_directory(file: &File) -> io::Result<()> {
    let value = info(file)?;
    if value.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
        || value.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(invalid("managed directory is not a plain directory"));
    }
    Ok(())
}
pub(super) fn open_directory(parent: &File, name: &Name, create: bool) -> io::Result<File> {
    let access = FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL;
    let file = match open(parent.as_raw_handle(), name.as_os_str(), access, create, true) {
        Err(e) if create && e.kind() == io::ErrorKind::AlreadyExists => {
            open(parent.as_raw_handle(), name.as_os_str(), access, false, true)?
        }
        result => result?,
    };
    plain_directory(&file)?;
    Ok(file)
}
pub(super) fn identity(file: &File) -> io::Result<FileIdentity> {
    let value = info(file)?;
    Ok(FileIdentity {
        volume: u64::from(value.dwVolumeSerialNumber),
        file: (u64::from(value.nFileIndexHigh) << 32) | u64::from(value.nFileIndexLow),
    })
}
pub(super) fn open_file(parent: &File, name: &Name, create: bool, write: bool) -> io::Result<File> {
    let access = FILE_READ_DATA
        | FILE_READ_ATTRIBUTES
        | READ_CONTROL
        | if write { FILE_WRITE_DATA | FILE_WRITE_ATTRIBUTES | DELETE } else { 0 };
    let file = open(parent.as_raw_handle(), name.as_os_str(), access, create, false)?;
    let value = info(&file)?;
    if value.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
        || value.nNumberOfLinks != 1
    {
        return Err(invalid("managed content must be a plain file with one link"));
    }
    Ok(file)
}
pub(super) fn sync_directory(_: &File) -> io::Result<()> {
    // Ordinary Windows directories have no documented flush contract. Files
    // use write-through and FlushFileBuffers; every startup reconciles intent
    // against actual namespace and complete generations before opening paths.
    Ok(())
}
pub(super) fn check_private(file: &File) -> io::Result<()> {
    super::windows_security::check(file)
}
pub(super) fn set_mode(file: &File, mode: u32) -> io::Result<()> {
    let mut p = file.metadata()?.permissions();
    p.set_readonly(mode & 0o200 == 0);
    file.set_permissions(p)
}
pub(super) fn rename_file(parent: &File, _from: &Name, file: &File, to: &Name, replace: bool) -> io::Result<()> {
    let name: Vec<u16> = to.as_os_str().encode_wide().chain([0]).collect();
    let total = offset_of!(FILE_RENAME_INFO, FileName) + name.len() * 2;
    let mut buffer = vec![0u64; total.div_ceil(8)];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // FileRenameInformationEx uses the same layout as FILE_RENAME_INFO; unlike
    // a full path it resolves one name against the held RootDirectory.
    // SAFETY: aligned/zeroed storage covers the complete terminated name.
    unsafe {
        (*info).Anonymous.Flags = 2 | if replace { 1 | 0x40 } else { 0 };
        (*info).RootDirectory = parent.as_raw_handle();
        (*info).FileNameLength = ((name.len() - 1) * 2) as u32;
        ptr::copy_nonoverlapping(name.as_ptr(), (&raw mut (*info).FileName).cast::<u16>(), name.len());
        let mut ios = IO_STATUS_BLOCK::default();
        status(NtSetInformationFile(file.as_raw_handle(), &mut ios, info.cast(), total as u32, 65))
    }
}

/// The only reparse handle exposed internally is the registered switch slot.
pub(crate) fn open_switch(parent: &File, name: &Name, create: bool) -> io::Result<File> {
    open(
        parent.as_raw_handle(),
        name.as_os_str(),
        FILE_READ_ATTRIBUTES | FILE_READ_DATA | FILE_WRITE_DATA | FILE_WRITE_ATTRIBUTES | DELETE | READ_CONTROL,
        create,
        true,
    )
}

pub(super) fn open_prepared(parent: &File, name: &Name) -> io::Result<File> {
    let file = open(
        parent.as_raw_handle(),
        name.as_os_str(),
        FILE_READ_DATA | FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES | DELETE | READ_CONTROL,
        false,
        false,
    )?;
    let value = info(&file)?;
    if value.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
        || value.nNumberOfLinks != 1
    {
        return Err(invalid("prepared content is not a plain file"));
    }
    Ok(file)
}
pub(super) fn confirm_prepared(_: &File) -> io::Result<()> {
    // The Runtime Prepared checkpoint attests to the earlier full data flush.
    // This reopen uses write-through for the remaining namespace operation.
    Ok(())
}
