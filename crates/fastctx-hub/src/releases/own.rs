//! One archive of the running Linux executable, retained only while the hub lives.

use std::fs::File;
use std::io::{self, Write};
use std::os::fd::AsRawFd as _;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest as _, Sha256};

use fastctx_types::kernel::{AARCH64_ARCHIVE, X86_64_ARCHIVE};

pub(super) struct Own {
    executable: File,
    archive: Mutex<Packed>,
}

/// A packing that failed is not kept: what failed it, a full disk or no
/// descriptor left, may have passed by the next asking, which packs again.
enum Packed {
    Not,
    Packed(Archive),
    Failed(String),
}

struct Archive {
    file: File,
    sums: File,
    digest: String,
}

pub(super) fn name() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some(X86_64_ARCHIVE),
        "aarch64" => Some(AARCH64_ARCHIVE),
        _ => None,
    }
}

impl Own {
    pub fn new() -> io::Result<Self> {
        // /proc/self/exe retains the running inode even after an upgrade replaces its path.
        Ok(Self { executable: File::open("/proc/self/exe")?, archive: Mutex::new(Packed::Not) })
    }

    pub fn digest(&self, directory: &Path) -> Result<String, String> {
        let mut archive = self.archive.lock().unwrap_or_else(PoisonError::into_inner);
        if let Packed::Packed(archive) = &*archive {
            return Ok(archive.digest.clone());
        }
        match self.pack(directory) {
            Ok(packed) => {
                let digest = packed.digest.clone();
                *archive = Packed::Packed(packed);
                Ok(digest)
            }
            Err(error) => {
                *archive = Packed::Failed(error.to_string());
                Err(error.to_string())
            }
        }
    }

    pub fn file(&self, sums: bool) -> io::Result<File> {
        match &*self.archive.lock().unwrap_or_else(PoisonError::into_inner) {
            Packed::Packed(archive) => reopen(if sums { &archive.sums } else { &archive.file }),
            Packed::Failed(error) => Err(io::Error::other(error.clone())),
            Packed::Not => Err(io::Error::new(io::ErrorKind::NotFound, "the running kernel has not been offered yet")),
        }
    }

    fn pack(&self, directory: &Path) -> io::Result<Archive> {
        // Unlinked storage leaves no old-version copies after a stop, crash or replacement.
        let file = tempfile::tempfile_in(directory)?;
        let writer = Hashed { file, hash: Sha256::new() };
        let mut archive = tar::Builder::new(GzEncoder::new(writer, Compression::fast()));
        for (name, contents) in fastctx_ripgrep::licenses::files() {
            append(&mut archive, name, contents.len() as u64, 0o644, contents.as_bytes())?;
        }
        let executable = reopen(&self.executable)?;
        append(&mut archive, "fastctx", executable.metadata()?.len(), 0o755, executable)?;
        let writer = archive.into_inner()?.finish()?;
        let digest = format!("{:x}", writer.hash.finalize());
        let mut sums = tempfile::tempfile_in(directory)?;
        writeln!(sums, "{digest}  {}", name().expect("only supported architectures are packed"))?;
        Ok(Archive { file: writer.file, sums, digest })
    }
}

fn append<W: Write>(
    archive: &mut tar::Builder<W>,
    name: &str,
    size: u64,
    mode: u32,
    contents: impl io::Read,
) -> io::Result<()> {
    let mut header = tar::Header::new_ustar();
    header.set_size(size);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    archive.append_data(&mut header, name, contents)
}

fn reopen(file: &File) -> io::Result<File> {
    // dup shares an offset; reopening gives simultaneous downloads independent cursors.
    File::open(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

struct Hashed {
    file: File,
    hash: Sha256,
}

impl Write for Hashed {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = self.file.write(bytes)?;
        self.hash.update(&bytes[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
