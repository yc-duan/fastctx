//! Files, for components whose threads have roles (`crate::roles`): each
//! call first notes the thread it is about to block, when that thread must
//! not block (`crate::conduct`), then does what `std::fs` does. A component
//! that keeps to its threads' roles reaches files only through here; its
//! `clippy.toml` refuses `std::fs` itself.

use std::fs::{self, File, Metadata, OpenOptions, ReadDir};
use std::io;
use std::path::{Path, PathBuf};

#[track_caller]
fn blocks() {
    crate::roles::blocking("disk");
}

#[track_caller]
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    blocks();
    fs::read(path)
}

#[track_caller]
pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    blocks();
    fs::read_to_string(path)
}

#[track_caller]
pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    blocks();
    fs::write(path, contents)
}

#[track_caller]
pub fn read_dir(path: impl AsRef<Path>) -> io::Result<ReadDir> {
    blocks();
    fs::read_dir(path)
}

#[track_caller]
pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    blocks();
    fs::create_dir_all(path)
}

#[track_caller]
pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    blocks();
    fs::remove_file(path)
}

#[track_caller]
pub fn remove_dir(path: impl AsRef<Path>) -> io::Result<()> {
    blocks();
    fs::remove_dir(path)
}

#[track_caller]
pub fn remove_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    blocks();
    fs::remove_dir_all(path)
}

#[track_caller]
pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    blocks();
    fs::rename(from, to)
}

#[track_caller]
pub fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<u64> {
    blocks();
    fs::copy(from, to)
}

#[track_caller]
pub fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    blocks();
    fs::metadata(path)
}

#[track_caller]
pub fn symlink_metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    blocks();
    fs::symlink_metadata(path)
}

#[track_caller]
pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    blocks();
    fs::canonicalize(path)
}

/// Whether `path` is there: as `std::fs::exists`, an error when it cannot
/// be told.
#[track_caller]
pub fn exists(path: impl AsRef<Path>) -> io::Result<bool> {
    blocks();
    fs::exists(path)
}

/// Whether `path` is a file, following links; false when it cannot be told,
/// as `Path::is_file`.
#[track_caller]
pub fn is_file(path: impl AsRef<Path>) -> bool {
    blocks();
    path.as_ref().is_file()
}

/// Whether `path` is a directory, following links; false when it cannot be
/// told, as `Path::is_dir`.
#[track_caller]
pub fn is_dir(path: impl AsRef<Path>) -> bool {
    blocks();
    path.as_ref().is_dir()
}

/// Whether `path` names something, following links; false when it cannot be
/// told, as `Path::exists`.
#[track_caller]
pub fn is_there(path: impl AsRef<Path>) -> bool {
    blocks();
    path.as_ref().exists()
}

#[track_caller]
pub fn open(path: impl AsRef<Path>) -> io::Result<File> {
    blocks();
    File::open(path)
}

#[track_caller]
pub fn create(path: impl AsRef<Path>) -> io::Result<File> {
    blocks();
    File::create(path)
}

/// Opens `path` as `options` say.
#[track_caller]
pub fn open_with(options: &OpenOptions, path: impl AsRef<Path>) -> io::Result<File> {
    blocks();
    options.open(path)
}

/// Creates another name for the same file, with the same role check as other IO.
#[track_caller]
pub fn hard_link(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    blocks();
    fs::hard_link(from, to)
}

/// Whether reading `path` is reading a file the kernel writes as it is
/// read, which never waits on a disk: under `/sys` or `/proc` on Linux,
/// except `/proc`'s links into the files and directories of processes.
fn kernel_written(path: &Path) -> bool {
    const LINKS: [&str; 5] = ["root", "cwd", "exe", "fd", "map_files"];
    cfg!(target_os = "linux")
        && (path.starts_with("/sys")
            || (path.starts_with("/proc")
                && !path.components().any(|part| LINKS.iter().any(|link| part.as_os_str() == *link))))
}

/// The text of a file the kernel writes as it is read (`/proc/net/route`),
/// which any thread may read; any other file is noted as
/// `read_to_string` notes it.
#[track_caller]
pub fn kernel_text(path: impl AsRef<Path>) -> io::Result<String> {
    if !kernel_written(path.as_ref()) {
        blocks();
    }
    fs::read_to_string(path)
}

/// Whether the kernel shows something at `path` (`/sys/class/net/eth0`),
/// which any thread may ask; any other path is noted as `exists` notes it.
#[track_caller]
pub fn kernel_exists(path: impl AsRef<Path>) -> io::Result<bool> {
    if !kernel_written(path.as_ref()) {
        blocks();
    }
    fs::exists(path)
}
