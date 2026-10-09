use std::io::{self, Read as _};
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
use std::sync::Arc;

pub(super) struct Wake(OwnedFd);

pub(super) struct Reader {
    pipe: io::PipeReader,
    pub wake: Arc<Wake>,
}

pub(super) fn pipe() -> io::Result<(Reader, io::PipeWriter)> {
    // SAFETY: eventfd returns a new descriptor with no inherited handles.
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if fd == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptor was just created and is owned here.
    let wake = Arc::new(Wake(unsafe { OwnedFd::from_raw_fd(fd) }));
    let (pipe, writer) = io::pipe()?;
    Ok((Reader { pipe, wake }, writer))
}

impl Wake {
    pub fn request(&self) -> io::Result<()> {
        let value = 1u64;
        loop {
            // SAFETY: eventfd takes exactly one u64 from a valid buffer.
            let n = unsafe { libc::write(self.0.as_raw_fd(), (&value as *const u64).cast(), 8) };
            if n == 8 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

impl Reader {
    pub fn next(&mut self, buffer: &mut [u8]) -> io::Result<super::Read> {
        if buffer.is_empty() {
            return Ok(super::Read::Bytes(0));
        }
        loop {
            let mut fds = [
                libc::pollfd { fd: self.wake.0.as_raw_fd(), events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: self.pipe.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            ];
            // SAFETY: both descriptors and the array are valid. No timeout:
            // output, EOF or the explicit checkpoint wakes this reader.
            if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, -1) } == -1 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if fds[0].revents & libc::POLLIN != 0 {
                let mut value = 0u64;
                // SAFETY: the event is readable and the buffer holds a u64.
                if unsafe { libc::read(fds[0].fd, (&mut value as *mut u64).cast(), 8) } == -1 {
                    return Err(io::Error::last_os_error());
                }
                let mut queued = 0;
                // SAFETY: FIONREAD writes one int, for this pipe's unread bytes.
                if unsafe { libc::ioctl(fds[1].fd, libc::FIONREAD, &mut queued) } == -1 {
                    return Err(io::Error::last_os_error());
                }
                return Ok(super::Read::Checkpoint(queued as usize));
            }
            if fds.iter().any(|fd| fd.revents & libc::POLLNVAL != 0) {
                return Err(io::Error::from_raw_os_error(libc::EBADF));
            }
            if fds[1].revents != 0 {
                return self.pipe.read(buffer).map(super::Read::Bytes);
            }
        }
    }
}
