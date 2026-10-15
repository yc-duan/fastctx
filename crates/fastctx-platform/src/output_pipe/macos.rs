use std::io::{self, Read as _};
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
use std::sync::Arc;

/// A pipe of its own that wakes the reader: macOS has no eventfd.
pub(super) struct Wake {
    read: OwnedFd,
    write: OwnedFd,
}

pub(super) struct Reader {
    pipe: io::PipeReader,
    pub wake: Arc<Wake>,
}

pub(super) fn pipe() -> io::Result<(Reader, io::PipeWriter)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: pipe fills the two-element array with new descriptors.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just created and are owned here.
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    for fd in [read.as_raw_fd(), write.as_raw_fd()] {
        // SAFETY: plain flag changes on descriptors owned here.
        unsafe {
            if libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) == -1
                || libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK) == -1
            {
                return Err(io::Error::last_os_error());
            }
        }
    }
    let wake = Arc::new(Wake { read, write });
    let (pipe, writer) = io::pipe()?;
    Ok((Reader { pipe, wake }, writer))
}

impl Wake {
    pub fn request(&self) -> io::Result<()> {
        loop {
            // SAFETY: one byte from a valid buffer.
            let n = unsafe { libc::write(self.write.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
            if n == 1 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::Interrupted => continue,
                // Full: a wake is already waiting to be read.
                io::ErrorKind::WouldBlock => return Ok(()),
                _ => return Err(error),
            }
        }
    }

    /// Takes every waiting wake.
    fn drain(&self) -> io::Result<()> {
        let mut buffer = [0u8; 64];
        loop {
            // SAFETY: reads into a buffer of its own length.
            let n = unsafe { libc::read(self.read.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
            if n > 0 {
                continue;
            }
            if n == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => return Ok(()),
                _ => return Err(error),
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
                libc::pollfd { fd: self.wake.read.as_raw_fd(), events: libc::POLLIN, revents: 0 },
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
                self.wake.drain()?;
                let mut queued: libc::c_int = 0;
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
