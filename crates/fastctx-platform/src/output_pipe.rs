//! A pipe reader that can establish a finite drain boundary without EOF.

use std::io;
use std::sync::Arc;

/// The sole reader of a process output stream.
pub struct Reader(imp::Reader);

/// A one-shot request to checkpoint the stream after its main writer exits.
#[derive(Clone)]
pub struct Checkpoint(Arc<imp::Wake>);

/// One read, or the bytes still queued when the reader observed a checkpoint.
pub enum Read {
    Bytes(usize),
    Checkpoint(usize),
}

/// Creates a byte stream with an interruptible reader and an ordinary writer.
pub fn pipe() -> io::Result<(Reader, io::PipeWriter)> {
    let (reader, writer) = imp::pipe()?;
    Ok((Reader(reader), writer))
}

impl Reader {
    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint(self.0.wake.clone())
    }

    /// A checkpoint is observed between reads, never while bytes are in
    /// flight. The caller must finish retaining the preceding read first.
    pub fn next(&mut self, buffer: &mut [u8]) -> io::Result<Read> {
        self.0.next(buffer)
    }
}

impl io::Read for Reader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            if let Read::Bytes(n) = self.next(buffer)? {
                return Ok(n);
            }
        }
    }
}

impl Checkpoint {
    pub fn request(&self) -> io::Result<()> {
        self.0.request()
    }
}

#[cfg(target_os = "linux")]
#[path = "output_pipe/linux.rs"]
mod imp;
#[cfg(target_os = "macos")]
#[path = "output_pipe/macos.rs"]
mod imp;
#[cfg(windows)]
#[path = "output_pipe/windows.rs"]
mod imp;
