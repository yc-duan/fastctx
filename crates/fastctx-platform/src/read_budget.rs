//! Optional, scoped bounds for components that read several configuration files.
use std::{
    cell::RefCell,
    fs::File,
    io::{self, Read},
    path::Path,
};

#[derive(Clone, Copy)]
pub struct Limits {
    pub file: usize,
    pub total: usize,
    pub files: usize,
}
struct Reading {
    limits: Limits,
    bytes: usize,
    files: usize,
    allocated: usize,
    failed: Option<String>,
}
thread_local! { static CURRENT: RefCell<Option<Reading>> = const { RefCell::new(None) }; }
struct Restore(Option<Reading>);
impl Drop for Restore {
    fn drop(&mut self) {
        CURRENT.with(|current| *current.borrow_mut() = self.0.take());
    }
}

/// Enforces bounds on this thread; even a limit error swallowed by `work` is returned.
/// A rejected result is dropped while further reads are still refused.
/// The prior scope is restored on return and on unwinding.
pub fn within<T>(limits: Limits, work: impl FnOnce() -> T) -> io::Result<T> {
    if CURRENT.with(|current| current.borrow().is_some()) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "a configuration read budget is already active"));
    }
    let _restore = Restore(
        CURRENT
            .with(|current| current.replace(Some(Reading { limits, bytes: 0, files: 0, allocated: 0, failed: None }))),
    );
    let value = work();
    match CURRENT.with(|current| current.borrow().as_ref().and_then(|reading| reading.failed.clone())) {
        Some(reason) => Err(io::Error::new(io::ErrorKind::InvalidData, reason)),
        None => Ok(value),
    }
}
fn failure(reading: &mut Reading, reason: String) -> io::Error {
    if reading.failed.is_none() {
        reading.failed = Some(reason.clone());
    }
    io::Error::new(io::ErrorKind::InvalidData, reason)
}
fn allowed(path: &Path) -> io::Result<Option<usize>> {
    CURRENT.with(|current| {
        let mut current = current.borrow_mut();
        let Some(reading) = current.as_mut() else { return Ok(None) };
        if let Some(reason) = &reading.failed {
            return Err(io::Error::new(io::ErrorKind::InvalidData, reason.clone()));
        }
        if reading.files >= reading.limits.files {
            let reason = format!("configuration reads exceed {} files at {}", reading.limits.files, path.display());
            return Err(failure(reading, reason));
        }
        reading.files += 1;
        Ok(Some(reading.limits.file.min(reading.limits.total.saturating_sub(reading.bytes))))
    })
}
pub fn active() -> bool {
    CURRENT.with(|current| current.borrow().is_some())
}

/// Checks metadata buffer requests before allocation, independently of file bytes.
/// One request uses the per-file limit and their sum uses the total limit.
pub fn allocation(path: &Path, bytes: usize) -> io::Result<()> {
    CURRENT.with(|current| {
        let mut current = current.borrow_mut();
        let Some(reading) = current.as_mut() else { return Ok(()) };
        if let Some(reason) = &reading.failed {
            return Err(io::Error::new(io::ErrorKind::InvalidData, reason.clone()));
        }
        let total = reading.allocated.saturating_add(bytes);
        if bytes > reading.limits.file || total > reading.limits.total {
            let reason = format!(
                "configuration metadata for {} exceeds the {} byte buffer or {} byte total allocation bound",
                path.display(),
                reading.limits.file,
                reading.limits.total
            );
            return Err(failure(reading, reason));
        }
        reading.allocated = total;
        Ok(())
    })
}
fn charge(bytes: usize) {
    CURRENT.with(|current| {
        if let Some(reading) = current.borrow_mut().as_mut() {
            reading.bytes = reading.bytes.saturating_add(bytes);
        }
    });
    crate::cost::read(bytes);
}
fn exceeded(path: &Path) -> io::Error {
    CURRENT.with(|current| {
        let mut current = current.borrow_mut();
        let reading = current.as_mut().expect("an active read budget");
        let reason = format!(
            "configuration file {} exceeds the {} byte per-file or {} byte total read bound",
            path.display(),
            reading.limits.file,
            reading.limits.total
        );
        failure(reading, reason)
    })
}
/// Reads an already-opened file, preserving the caller's identity checks.
pub fn from_reader(reader: &mut impl Read, path: &Path) -> io::Result<Vec<u8>> {
    let Some(bound) = allowed(path)? else {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        crate::cost::read(bytes.len());
        return Ok(bytes);
    };
    bounded(reader, path, bound)
}
fn bounded(reader: &mut impl Read, path: &Path, bound: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    while bytes.len() < bound {
        let capacity = (bound - bytes.len()).min(chunk.len());
        let got = reader.read(&mut chunk[..capacity])?;
        charge(got);
        if got == 0 {
            return Ok(bytes);
        }
        if bytes.len() + got > bytes.capacity() {
            bytes.reserve_exact(got);
        }
        bytes.extend_from_slice(&chunk[..got]);
    }
    let mut probe = [0u8; 1];
    let got = reader.read(&mut probe)?;
    charge(got);
    if got != 0 {
        return Err(exceeded(path));
    }
    Ok(bytes)
}
/// Reads normally outside a scope, and only regular bounded files inside one.
pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    let Some(bound) = allowed(path)? else {
        return std::fs::read(path).inspect(|bytes| crate::cost::read(bytes.len()));
    };
    let mut file: File = crate::disk::open_regular(path)?;
    bounded(&mut file, path, bound)
}
pub fn read_to_string(path: &Path) -> io::Result<String> {
    if CURRENT.with(|current| current.borrow().is_none()) {
        return std::fs::read_to_string(path).inspect(|text| crate::cost::read(text.len()));
    }
    String::from_utf8(read(path)?).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
