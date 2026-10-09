use std::io;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NOT_FOUND, ERROR_OPERATION_ABORTED, GENERIC_WRITE, INVALID_HANDLE_VALUE,
    WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, PIPE_ACCESS_INBOUND, ReadFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PeekNamedPipe};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};

pub(super) struct Wake(OwnedHandle);

pub(super) struct Reader {
    pipe: OwnedHandle,
    ready: OwnedHandle,
    pub wake: Arc<Wake>,
}

fn event() -> io::Result<OwnedHandle> {
    // SAFETY: a new unnamed manual-reset event with no inherited handle.
    let raw = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the event was just created and is owned here.
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

pub(super) fn pipe() -> io::Result<(Reader, io::PipeWriter)> {
    // Anonymous Win32 pipes cannot perform overlapped reads. A single-instance
    // local byte pipe keeps the ordinary writer semantics while allowing the
    // reader to wait for either output or a checkpoint without polling.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = format!(r"\\.\pipe\fastctx-output-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: the name is terminated; no security attributes are inherited.
    let raw = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_INBOUND | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_REJECT_REMOTE_CLIENTS,
            1,
            0,
            0,
            0,
            ptr::null(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the pipe was just created and is owned here.
    let pipe = unsafe { OwnedHandle::from_raw_handle(raw) };
    // SAFETY: opens this pipe's sole client, with synchronous writes.
    let raw = unsafe { CreateFileW(name.as_ptr(), GENERIC_WRITE, 0, ptr::null(), OPEN_EXISTING, 0, ptr::null_mut()) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the writing handle was just opened and is owned here.
    let writer = io::PipeWriter::from(unsafe { OwnedHandle::from_raw_handle(raw) });
    Ok((Reader { pipe, ready: event()?, wake: Arc::new(Wake(event()?)) }, writer))
}

impl Wake {
    pub fn request(&self) -> io::Result<()> {
        // SAFETY: a valid event handle.
        if unsafe { SetEvent(self.0.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Reader {
    pub fn next(&mut self, buffer: &mut [u8]) -> io::Result<super::Read> {
        if buffer.is_empty() {
            return Ok(super::Read::Bytes(0));
        }
        loop {
            // SAFETY: a zero wait on a valid event handle.
            match unsafe { WaitForSingleObject(self.wake.0.as_raw_handle(), 0) } {
                WAIT_OBJECT_0 => return self.checkpoint().map(super::Read::Checkpoint),
                WAIT_TIMEOUT => {}
                _ => return Err(io::Error::last_os_error()),
            }
            // SAFETY: initialized for one read, and retained until it completes.
            let mut operation: OVERLAPPED = unsafe { std::mem::zeroed() };
            operation.hEvent = self.ready.as_raw_handle();
            // SAFETY: the event belongs to this reader and no operation is pending.
            if unsafe { ResetEvent(operation.hEvent) } == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: buffer and operation remain live until completion below.
            let result = unsafe {
                ReadFile(
                    self.pipe.as_raw_handle(),
                    buffer.as_mut_ptr(),
                    buffer.len().min(u32::MAX as usize) as u32,
                    ptr::null_mut(),
                    &mut operation,
                )
            };
            if result == 0 {
                let error = io::Error::last_os_error();
                match error.raw_os_error().map(|n| n as u32) {
                    Some(ERROR_BROKEN_PIPE) => return Ok(super::Read::Bytes(0)),
                    Some(ERROR_IO_PENDING) => {}
                    _ => return Err(error),
                }
                let handles = [self.wake.0.as_raw_handle(), operation.hEvent];
                // SAFETY: two valid handles, waiting for either with no timeout.
                let ready = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
                if ready == WAIT_OBJECT_0 || ready == WAIT_FAILED {
                    let wait_error = (ready == WAIT_FAILED).then(io::Error::last_os_error);
                    // SAFETY: cancels only this reader's outstanding operation.
                    let cancelled = unsafe { CancelIoEx(self.pipe.as_raw_handle(), &operation) };
                    let cancel_error = (cancelled == 0).then(io::Error::last_os_error);
                    // Cancellation races successful reads. Always collect the
                    // result, retaining any consumed bytes before checkpointing.
                    let read = self.completed(&mut operation);
                    if let Some(error) = wait_error {
                        return Err(error);
                    }
                    if let Some(error) = cancel_error
                        && error.raw_os_error() != Some(ERROR_NOT_FOUND as i32)
                    {
                        return Err(error);
                    }
                    match read {
                        Err(error) if error.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32) => continue,
                        result => return result.map(super::Read::Bytes),
                    }
                }
            }
            return self.completed(&mut operation).map(super::Read::Bytes);
        }
    }

    fn completed(&self, operation: &mut OVERLAPPED) -> io::Result<usize> {
        let mut read = 0;
        // SAFETY: the operation and its buffer remain alive; wait for completion
        // even after cancellation before either storage can be released.
        if unsafe { GetOverlappedResult(self.pipe.as_raw_handle(), operation, &mut read, 1) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_BROKEN_PIPE as i32) {
                return Err(error);
            }
        }
        Ok(read as usize)
    }

    fn checkpoint(&self) -> io::Result<usize> {
        // SAFETY: the one-shot request has been observed.
        if unsafe { ResetEvent(self.wake.0.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut queued = 0;
        // SAFETY: no read is outstanding, and queued is a valid out-parameter.
        if unsafe {
            PeekNamedPipe(self.pipe.as_raw_handle(), ptr::null_mut(), 0, ptr::null_mut(), &mut queued, ptr::null_mut())
        } == 0
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_BROKEN_PIPE as i32) {
                return Err(error);
            }
        }
        Ok(queued as usize)
    }
}
