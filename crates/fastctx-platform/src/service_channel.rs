//! The private, inherited channel between a Windows guardian and its node.
//! Each message is exactly one byte. It carries lifecycle state only, never
//! a path, identity request, command line, or executable code.

use std::io::{self, Read, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ServiceCommand {
    Activate = 0x01,
    YieldWhenIdle = 0x02,
    CancelYield = 0x03,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ServiceEvent {
    Prepared = 0x11,
    Serving = 0x12,
    Yielded = 0x13,
}

impl ServiceCommand {
    pub fn read(reader: &mut impl Read) -> io::Result<Option<Self>> {
        read(reader)?
            .map(|value| match value {
                0x01 => Ok(Self::Activate),
                0x02 => Ok(Self::YieldWhenIdle),
                0x03 => Ok(Self::CancelYield),
                _ => Err(io::Error::new(io::ErrorKind::InvalidData, "unknown service command")),
            })
            .transpose()
    }

    pub fn write(self, writer: &mut impl Write) -> io::Result<()> {
        writer.write_all(&[self as u8])
    }
}

impl ServiceEvent {
    pub fn read(reader: &mut impl Read) -> io::Result<Option<Self>> {
        read(reader)?
            .map(|value| match value {
                0x11 => Ok(Self::Prepared),
                0x12 => Ok(Self::Serving),
                0x13 => Ok(Self::Yielded),
                _ => Err(io::Error::new(io::ErrorKind::InvalidData, "unknown service event")),
            })
            .transpose()
    }

    pub fn write(self, writer: &mut impl Write) -> io::Result<()> {
        writer.write_all(&[self as u8])
    }
}

fn read(reader: &mut impl Read) -> io::Result<Option<u8>> {
    let mut byte = [0];
    match reader.read_exact(&mut byte) {
        Ok(()) => Ok(Some(byte[0])),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
        Err(error) => Err(error),
    }
}

/// The two ends a guardian handed down to its node, by the handle numbers
/// on the node's command line: the one it reads commands from, and the one
/// it writes events to. They stop being inheritable at once, so nothing the
/// node starts receives them.
#[cfg(windows)]
pub fn inherited(control: u64, events: u64) -> io::Result<(std::fs::File, std::fs::File)> {
    use std::os::windows::io::{FromRawHandle, RawHandle};
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};

    let take = |value: u64| -> io::Result<std::fs::File> {
        let handle = value as usize as RawHandle;
        // SAFETY: the guardian passed this handle to this process alone,
        // by inheritance; from here on this process owns it.
        if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { std::fs::File::from_raw_handle(handle) })
    };
    Ok((take(control)?, take(events)?))
}

/// A guardian hands its node a channel only on Windows; elsewhere the
/// system's service manager is the guardian and needs none.
#[cfg(not(windows))]
pub fn inherited(_control: u64, _events: u64) -> io::Result<(std::fs::File, std::fs::File)> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "a guardian hands its node a channel only on Windows"))
}
