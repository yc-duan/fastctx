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

    /// A job supervisor's same-user-only endpoint. The PID locates it;
    /// callers must also check Connection::server_process against the
    /// recorded identity, since a PID can be reused.
    pub fn for_supervisor(world_dir: &Path, pid: u32) -> io::Result<Self> {
        imp::supervisor_address(world_dir, pid).map(Self)
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
    server: Option<fastctx_types::ProcessIdentity>,
}

impl Connection {
    fn new(stream: impl AsyncRead + AsyncWrite + Send + Unpin + 'static) -> Self {
        let (reader, writer) = tokio::io::split(Box::new(stream) as Box<dyn Stream>);
        Self {
            server: None,
            reader: FrameReader { stream: reader, header: [0; 4], header_filled: 0, length: None, frame: Vec::new() },
            writer: FrameWriter {
                stream: writer,
                #[cfg(windows)]
                pipe: None,
            },
        }
    }

    /// The OS identity of the local server, when the platform exposes it.
    /// This is used to retain a legacy node's actual image before renewal.
    pub fn server_process(&self) -> Option<&fastctx_types::ProcessIdentity> {
        self.server.as_ref()
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

pub struct FrameReader {
    stream: ReadHalf<Box<dyn Stream>>,
    header: [u8; 4],
    header_filled: usize,
    length: Option<usize>,
    frame: Vec<u8>,
}

impl FrameReader {
    /// The next frame, or `None` if the other side closed the connection
    /// between frames. Closing in the middle of a frame is an error.
    /// Cancelling a read preserves its partial header and payload.
    pub async fn recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        self.recv_up_to(u32::MAX as usize).await
    }

    /// Receives one frame whose size is bounded by the current protocol
    /// phase, before allocating it. This does not limit a stream's total.
    pub async fn recv_up_to(&mut self, maximum: usize) -> io::Result<Option<Vec<u8>>> {
        while self.header_filled < self.header.len() {
            match self.stream.read(&mut self.header[self.header_filled..]).await? {
                0 if self.header_filled == 0 => return Ok(None),
                0 => return Err(io::ErrorKind::UnexpectedEof.into()),
                read => self.header_filled += read,
            }
        }
        let length = *self.length.get_or_insert_with(|| u32::from_le_bytes(self.header) as usize);
        if length > maximum {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "frame exceeds this protocol phase's size limit"));
        }
        // The peer's length is a boundary, not evidence that its bytes exist.
        // Grow only for bytes actually received, including on a truncated frame.
        while self.frame.len() < length {
            let remaining = length - self.frame.len();
            if (&mut self.stream).take(remaining as u64).read_buf(&mut self.frame).await? == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
        }
        self.header_filled = 0;
        self.length = None;
        Ok(Some(std::mem::take(&mut self.frame)))
    }
}

pub struct FrameWriter {
    stream: WriteHalf<Box<dyn Stream>>,
    #[cfg(windows)]
    pipe: Option<std::sync::Arc<std::fs::File>>,
}

impl FrameWriter {
    pub async fn send(&mut self, frame: &[u8]) -> io::Result<()> {
        let length = u32::try_from(frame.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a frame is larger than 4 GiB"))?;
        let mut bytes = Vec::with_capacity(4 + frame.len());
        bytes.extend_from_slice(&length.to_le_bytes());
        bytes.extend_from_slice(frame);
        self.stream.write_all(&bytes).await?;
        self.stream.flush().await
    }

    /// Finishes a response before its server may exit. Tokio's named-pipe
    /// flush does not wait for the client to consume the buffered bytes;
    /// closing the process with those bytes pending can lose the response.
    /// On Windows server pipes, FlushFileBuffers waits for that consumption.
    pub async fn drain(&mut self) -> io::Result<()> {
        #[cfg(windows)]
        if let Some(pipe) = &self.pipe {
            use std::os::windows::io::AsRawHandle as _;
            use windows_sys::Win32::Storage::FileSystem::FlushFileBuffers;

            let pipe = pipe.clone();
            tokio::task::spawn_blocking(move || {
                // SAFETY: the owned duplicate keeps this server pipe valid
                // until the blocking flush completes or its client closes.
                if unsafe { FlushFileBuffers(pipe.as_raw_handle()) } == 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            })
            .await
            .map_err(io::Error::other)??;
        }
        Ok(())
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
    pub struct Address {
        path: PathBuf,
        same_user: bool,
    }

    impl fmt::Display for Address {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.path.display().fmt(f)
        }
    }

    pub fn address(world_dir: &Path) -> io::Result<Address> {
        Ok(Address { path: world_dir.join("node.sock"), same_user: false })
    }

    pub fn supervisor_address(world_dir: &Path, pid: u32) -> io::Result<Address> {
        Ok(Address { path: world_dir.join(format!("job-{pid}.sock")), same_user: true })
    }

    pub struct Listener {
        socket: UnixListener,
        address: Address,
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            if self.address.same_user {
                // Only this supervisor uses its PID endpoint while alive.
                let _ = std::fs::remove_file(&self.address.path);
            }
        }
    }

    /// macOS answers a question about the other end of a connection that end
    /// closed already with ENOTCONN, where Linux still answers it: a
    /// connection gone before it was looked at was reset.
    fn gone(error: io::Error) -> io::Error {
        if error.kind() == io::ErrorKind::NotConnected {
            io::Error::new(io::ErrorKind::ConnectionReset, "the other end closed the connection as it was made")
        } else {
            error
        }
    }

    fn same_user(stream: &UnixStream) -> io::Result<()> {
        // SAFETY: geteuid has no preconditions.
        if stream.peer_cred().map_err(gone)?.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the supervisor connection belongs to another user",
            ));
        }
        Ok(())
    }

    impl Listener {
        pub fn bind(address: &Address) -> io::Result<Self> {
            match std::fs::remove_file(&address.path) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(with_place(error, address)),
                _ => {}
            }
            UnixListener::bind(&address.path)
                .map(|listener| Self { socket: listener, address: address.clone() })
                .map_err(|error| with_place(error, address))
        }

        pub async fn accept(&mut self) -> io::Result<Connection> {
            loop {
                let (stream, _) = self.socket.accept().await?;
                if self.address.same_user {
                    match same_user(&stream) {
                        Ok(()) => {}
                        // A client that left before it was looked at is
                        // not served; the next one is waited for.
                        Err(error) if error.kind() == io::ErrorKind::ConnectionReset => continue,
                        Err(error) => return Err(error),
                    }
                }
                return Ok(Connection::new(stream));
            }
        }
    }

    pub async fn connect(address: &Address) -> io::Result<Connection> {
        let stream = UnixStream::connect(&address.path).await.map_err(|error| with_place(error, address))?;
        if address.same_user {
            same_user(&stream).map_err(|error| with_place(error, address))?;
        }
        let identity = stream
            .peer_cred()
            .map_err(|error| with_place(gone(error), address))?
            .pid()
            .map(|pid| crate::identity_of(pid as u32))
            .transpose()?
            .flatten();
        let mut connection = Connection::new(stream);
        connection.server = identity;
        Ok(connection)
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
    pub struct Address {
        name: String,
        same_user: bool,
    }

    impl fmt::Display for Address {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.name.fmt(f)
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
        Ok(Address { name: format!(r"\\.\pipe\fastctx-node-{hash:016x}"), same_user: false })
    }

    pub fn supervisor_address(world_dir: &Path, pid: u32) -> io::Result<Address> {
        let world = address(world_dir)?;
        Ok(Address { name: format!("{}-job-{pid}", world.name), same_user: true })
    }

    pub struct Listener {
        address: Address,
        access: OwnerOnly,
        /// The instance waiting for the next client.
        next: Option<NamedPipeServer>,
    }

    impl Listener {
        pub fn bind(address: &Address) -> io::Result<Self> {
            let access = if address.same_user { OwnerOnly::user_only()? } else { OwnerOnly::new(false)? };
            let mut listener = Self { address: address.clone(), access, next: None };
            listener.next = Some(listener.instance(true).map_err(|error| with_place(error, address))?);
            Ok(listener)
        }

        pub async fn accept(&mut self) -> io::Result<Connection> {
            if self.next.is_none() {
                self.next = Some(self.instance(false)?);
            }
            // The watchdog may cancel this wait. Keep the pipe in the
            // listener until connected, including any client arriving then.
            if let Err(error) = self.next.as_ref().expect("a listening instance exists").connect().await {
                self.next = None;
                return Err(error);
            }
            let server = self.next.take().expect("the connected instance");
            // The next instance is made before this one is handed out, so a
            // client never finds the pipe missing. If it cannot be made, this
            // connection is still handed out and the next accept reports why.
            self.next = self.instance(false).ok();
            use std::os::windows::io::BorrowedHandle;
            // SAFETY: server owns this handle through the duplication.
            let handle = unsafe { BorrowedHandle::borrow_raw(server.as_raw_handle()) }.try_clone_to_owned()?;
            let mut connection = Connection::new(server);
            connection.writer.pipe = Some(std::sync::Arc::new(std::fs::File::from(handle)));
            Ok(connection)
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
                    .create_with_security_attributes_raw(&self.address.name, (&raw mut attributes).cast())
            }
        }
    }

    pub async fn connect(address: &Address) -> io::Result<Connection> {
        let client = loop {
            match ClientOptions::new().open(&address.name) {
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
        let mut pid = 0;
        if unsafe { windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId(client.as_raw_handle(), &mut pid) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        let identity = crate::identity_of(pid)?;
        let mut connection = Connection::new(client);
        connection.server = identity;
        Ok(connection)
    }
}
