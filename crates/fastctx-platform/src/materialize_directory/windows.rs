//! Retarget the registered junction itself, never delete-then-create on a live path.
use super::{DirectoryBoundary, DirectoryCheckpoint, DirectoryJournal, DirectoryPlan, invalid};
use crate::{
    materialize_file::{Failure, Visibility},
    rooted_path::{Directory, FileIdentity, identity, windows::open_switch},
};
use std::{
    fs::File,
    io,
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
    ptr,
};
use windows_sys::Win32::{
    Globalization::{CSTR_EQUAL, CompareStringOrdinal},
    Storage::FileSystem::FlushFileBuffers,
    System::IO::DeviceIoControl,
};
const MOUNT_POINT: u32 = 0xa0000003;
const GET_REPARSE: u32 = 0x000900a8;
const SET_REPARSE: u32 = 0x000900a4;

pub(super) fn switch(
    parent: &Directory,
    plan: &DirectoryPlan,
    journal: &mut impl DirectoryJournal,
    recovering: bool,
) -> Result<FileIdentity, Failure> {
    let mut created = false;
    let file = match open_switch(parent.file(), &plan.stable, false) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if !recovering && plan.slot.is_some() {
                return Err(Failure::unchanged(invalid("registered junction is missing")));
            }
            if plan.slot.is_some() {
                journal.record(plan, DirectoryCheckpoint::SlotRepair).map_err(Failure::unchanged)?;
            }
            let file = open_switch(parent.file(), &plan.stable, true).map_err(Failure::unchanged)?;
            journal.observed(plan, DirectoryBoundary::SlotCreated);
            created = true;
            file
        }
        Err(e) => return Err(Failure::unchanged(e)),
    };
    let slot = identity(&file).map_err(Failure::unchanged)?;
    if !created && plan.slot.is_some_and(|expected| expected != slot) {
        return Err(Failure::unchanged(invalid("registered junction identity changed")));
    }
    if created {
        journal.record(plan, DirectoryCheckpoint::SlotCreated { identity: slot }).map_err(Failure::unchanged)?;
    }
    let wanted = target(parent, &plan.wanted.name)?;
    let present = read_target(&file).map_err(Failure::unchanged)?;
    if let Some(present) = present {
        let matches_wanted = same_target(&present, &wanted);
        let matches_previous = plan
            .previous
            .as_ref()
            .map(|g| target(parent, &g.name))
            .transpose()?
            .is_some_and(|old| same_target(&present, &old));
        // Startup may repair a registered slot whose target was changed. The
        // unexpected target is inspected as reparse bytes and never traversed.
        if !matches_wanted && !matches_previous && (!recovering || plan.slot != Some(slot)) {
            return Err(Failure::unchanged(invalid("junction points outside its recorded generations")));
        }
        if matches_wanted {
            flush(&file).map_err(|e| Failure::new(e, Visibility::Published))?;
            journal.observed(plan, DirectoryBoundary::JunctionFlushed);
            return Ok(slot);
        }
    } else if !created && !recovering {
        return Err(Failure::unchanged(invalid("registered switch slot lost its reparse data")));
    }
    // Recovery of the first installation can reuse the empty slot reserved by
    // its durable Intent. Existing content is never removed to make it empty.
    set_target(&file, &wanted).map_err(|e| Failure::new(e, Visibility::Unknown))?;
    journal.observed(plan, DirectoryBoundary::NamespaceChanged);
    flush(&file).map_err(|e| Failure::new(e, Visibility::Published))?;
    journal.observed(plan, DirectoryBoundary::JunctionFlushed);
    let actual = read_target(&file).map_err(|e| Failure::new(e, Visibility::Published))?;
    if actual.as_ref().is_none_or(|v| !same_target(v, &wanted)) {
        return Err(Failure::new(invalid("junction target did not verify"), Visibility::Published));
    }
    Ok(slot)
}
fn target(parent: &Directory, name: &crate::rooted_path::Name) -> Result<Vec<u16>, Failure> {
    let path = parent.display_path().join(name.as_os_str());
    let path = path.as_os_str().encode_wide().collect::<Vec<_>>();
    let mut out = "\\??\\".encode_utf16().collect::<Vec<_>>();
    out.extend_from_slice(&path);
    if out.len() > 4000 {
        return Err(Failure::unchanged(invalid("junction target exceeds reparse buffer bound")));
    }
    Ok(out)
}
fn same_target(left: &[u16], right: &[u16]) -> bool {
    // SAFETY: explicit lengths describe both valid UTF-16 code-unit arrays.
    unsafe {
        CompareStringOrdinal(left.as_ptr(), left.len() as i32, right.as_ptr(), right.len() as i32, 1) == CSTR_EQUAL
    }
}
fn flush(file: &File) -> io::Result<()> {
    // SAFETY: the switch handle has write access and is synchronous.
    if unsafe { FlushFileBuffers(file.as_raw_handle()) } == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}
fn read_target(file: &File) -> io::Result<Option<Vec<u16>>> {
    let mut bytes = vec![0u8; 16 * 1024];
    let mut count = 0;
    // SAFETY: bounded writable output and a synchronous live reparse handle.
    if unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            GET_REPARSE,
            ptr::null(),
            0,
            bytes.as_mut_ptr().cast(),
            bytes.len() as u32,
            &mut count,
            ptr::null_mut(),
        )
    } == 0
    {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(4390) {
            return Ok(None);
        }
        return Err(error);
    }
    bytes.truncate(count as usize);
    if bytes.len() < 16 || u32::from_le_bytes(bytes[..4].try_into().unwrap()) != MOUNT_POINT {
        return Err(invalid("switch slot is not a mount-point reparse record"));
    }
    let data = u16::from_le_bytes(bytes[4..6].try_into().unwrap()) as usize;
    let offset = u16::from_le_bytes(bytes[8..10].try_into().unwrap()) as usize;
    let length = u16::from_le_bytes(bytes[10..12].try_into().unwrap()) as usize;
    if data + 8 != bytes.len()
        || !offset.is_multiple_of(2)
        || !length.is_multiple_of(2)
        || length == 0
        || 16 + offset + length > bytes.len()
    {
        return Err(invalid("malformed junction target"));
    }
    Ok(Some(
        bytes[16 + offset..16 + offset + length].chunks_exact(2).map(|v| u16::from_le_bytes([v[0], v[1]])).collect(),
    ))
}
fn set_target(file: &File, wanted: &[u16]) -> io::Result<()> {
    let substitute_bytes = wanted.len() * 2;
    let shown = &wanted[4..];
    let data_length = 8 + substitute_bytes + 2 + shown.len() * 2 + 2;
    let mut bytes = Vec::with_capacity(8 + data_length);
    bytes.extend_from_slice(&MOUNT_POINT.to_le_bytes());
    bytes.extend_from_slice(&(data_length as u16).to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    for value in [0u16, substitute_bytes as u16, (substitute_bytes + 2) as u16, (shown.len() * 2) as u16] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in wanted.iter().copied().chain([0]).chain(shown.iter().copied()).chain([0]) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let mut returned = 0;
    // SAFETY: the checked bounded mount-point buffer lives throughout the call.
    if unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            SET_REPARSE,
            bytes.as_ptr().cast(),
            bytes.len() as u32,
            ptr::null_mut(),
            0,
            &mut returned,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
