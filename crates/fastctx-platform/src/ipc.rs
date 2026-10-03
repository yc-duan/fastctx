//! The local endpoint where the node listens and its clients connect.
//!
//! There is one per World. On Unix it is a socket inside the World's
//! private directory. On Windows it is a named pipe whose name comes from
//! the directory's canonical path, so every spelling of the same directory
//! reaches the same node. Pipe names are global on Windows and anyone may
//! create one first: the node's pipe is owned by its user and admits only
//! that user, and a client refuses a pipe owned by anyone else.
//!
//! A connection carries frames: a little-endian `u32` length, then that many
//! bytes.

use std::fmt;
use std::io;
use std::path::Path;

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, ReadHalf, WriteHalf};

/// Where the node of one World listens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint(imp::Address);

impl Endpoint {
    /// The endpoint of the World kept in `world_dir`, which must exist.
    pub fn for_world(world_dir: &Path) -> io::Result<Self> {
        imp::address(world_dir).map(Self)
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The node's side of the endpoint.
pub struct Listener(imp::Listener);

impl Listener {
    /// Listens at `endpoint`. The caller holds the World's node lock: a
    /// socket that a node left behind when it died is replaced.
    pub fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        imp::Listener::bind(&endpoint.0).map(Self)
    }

    pub async fn accept(&mut self) -> io::Result<Connection> {
        self.0.accept().await
    }
}

/// Connects to the node listening at `endpoint`. `NotFound` and
/// `ConnectionRefused` mean that no node is listening there.
pub async fn connect(endpoint: &Endpoint) -> io::Result<Connection> {
    imp::connect(&endpoint.0).await
}

trait Stream: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin> Stream for T {}

/// One client's connection to the node, from either side.
pub struct Connection {
    reader: FrameReader,
    writer: FrameWriter,
}

impl Connection {
    fn new(stream: impl AsyncRead + AsyncWrite + Send + Unpin + 'static) -> Self {
        let (reader, writer) = tokio::io::split(Box::new(stream) as Box<dyn Stream>);
        Self { reader: FrameReader(reader), writer: FrameWriter(writer) }
    }

    /// Separates the two directions, so one task can read while another
    /// writes.
    pub fn split(self) -> (FrameReader, FrameWriter) {
        (self.reader, self.writer)
    }

    pub async fn send(&mut self, frame: &[u8]) -> io::Result<()> {
        self.writer.send(frame).await
    }

    pub async fn recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        self.reader.recv().await
    }
}

pub struct FrameReader(ReadHalf<Box<dyn Stream>>);

impl FrameReader {
    /// The next frame, or `None` if the other side closed the connection
    /// between frames. Closing in the middle of a frame is an error.
    pub async fn recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut length = [0u8; 4];
        let mut filled = 0;
        while filled < length.len() {
            match self.0.read(&mut length[filled..]).await? {
                0 if filled == 0 => return Ok(None),
                0 => return Err(io::ErrorKind::UnexpectedEof.into()),
                read => filled += read,
            }
        }
        let mut frame = vec![0u8; u32::from_le_bytes(length) as usize];
        self.0.read_exact(&mut frame).await?;
        Ok(Some(frame))
    }
}

pub struct FrameWriter(WriteHalf<Box<dyn Stream>>);

impl FrameWriter {
    pub async fn send(&mut self, frame: &[u8]) -> io::Result<()> {
        let length = u32::try_from(frame.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a frame is larger than 4 GiB"))?;
        let mut bytes = Vec::with_capacity(4 + frame.len());
        bytes.extend_from_slice(&length.to_le_bytes());
        bytes.extend_from_slice(frame);
        self.0.write_all(&bytes).await?;
        self.0.flush().await
    }
}

fn with_place(error: io::Error, place: &dyn fmt::Display) -> io::Error {
    io::Error::new(error.kind(), format!("{place}: {error}"))
}

#[cfg(unix)]
mod imp {
    use std::fmt;
    use std::io;
    use std::path::{Path, PathBuf};

    use tokio::net::{UnixListener, UnixStream};

    use super::{Connection, with_place};

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Address(PathBuf);

    impl fmt::Display for Address {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.display().fmt(f)
        }
    }

    pub fn address(world_dir: &Path) -> io::Result<Address> {
        Ok(Address(world_dir.join("node.sock")))
    }

    pub struct Listener(UnixListener);

    impl Listener {
        pub fn bind(address: &Address) -> io::Result<Self> {
            match std::fs::remove_file(&address.0) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(with_place(error, address)),
                _ => {}
            }
            UnixListener::bind(&address.0).map(Self).map_err(|error| with_place(error, address))
        }

        pub async fn accept(&mut self) -> io::Result<Connection> {
            let (stream, _) = self.0.accept().await?;
            Ok(Connection::new(stream))
        }
    }

    pub async fn connect(address: &Address) -> io::Result<Connection> {
        UnixStream::connect(&address.0).await.map(Connection::new).map_err(|error| with_place(error, address))
    }
}

#[cfg(windows)]
mod imp {
    use std::fmt;
    use std::io;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use std::path::Path;
    use std::time::Duration;

    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
    use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;

    use super::{Connection, with_place};
    use crate::windows_user::{OwnerOnly, current_user_sid, object_owner_sid};

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Address(String);

    impl fmt::Display for Address {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.fmt(f)
        }
    }

    pub fn address(world_dir: &Path) -> io::Result<Address> {
        // The canonical path is the one spelling the file system itself
        // reports, whatever drive-letter case or separators a shell used.
        let canonical = std::fs::canonicalize(world_dir).map_err(|error| with_place(error, &world_dir.display()))?;
        // FNV-1a over the UTF-16 units: stable across builds and versions,
        // so every fastctx binary finds the same pipe.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for unit in canonical.as_os_str().encode_wide() {
            for byte in unit.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
        }
        Ok(Address(format!(r"\\.\pipe\fastctx-node-{hash:016x}")))
    }

    pub struct Listener {
        address: Address,
        access: OwnerOnly,
        /// The instance waiting for the next client.
        next: Option<NamedPipeServer>,
    }

    impl Listener {
        pub fn bind(address: &Address) -> io::Result<Self> {
            let mut listener = Self { address: address.clone(), access: OwnerOnly::new(false)?, next: None };
            listener.next = Some(listener.instance(true).map_err(|error| with_place(error, address))?);
            Ok(listener)
        }

        pub async fn accept(&mut self) -> io::Result<Connection> {
            let server = match self.next.take() {
                Some(server) => server,
                None => self.instance(false)?,
            };
            server.connect().await?;
            // The next instance is made before this one is handed out, so a
            // client never finds the pipe missing. If it cannot be made, this
            // connection is still handed out and the next accept reports why.
            self.next = self.instance(false).ok();
            Ok(Connection::new(server))
        }

        fn instance(&self, first: bool) -> io::Result<NamedPipeServer> {
            let mut attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: self.access.descriptor(),
                bInheritHandle: 0,
            };
            // SAFETY: the attributes point to a valid descriptor that
            // outlives the call.
            unsafe {
                ServerOptions::new()
                    .first_pipe_instance(first)
                    .reject_remote_clients(true)
                    .create_with_security_attributes_raw(&self.address.0, (&raw mut attributes).cast())
            }
        }
    }

    pub async fn connect(address: &Address) -> io::Result<Connection> {
        let client = loop {
            match ClientOptions::new().open(&address.0) {
                Ok(client) => break client,
                // Every instance is taken for the moment between the node
                // accepting one client and making the next instance.
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                    tokio::time::sleep(Duration::from_millis(1)).await
                }
                Err(error) => return Err(with_place(error, address)),
            }
        };
        let owner = object_owner_sid(client.as_raw_handle())?;
        if owner != current_user_sid()? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{address} is held by another user ({owner}), not by this user's node"),
            ));
        }
        Ok(Connection::new(client))
    }
}
