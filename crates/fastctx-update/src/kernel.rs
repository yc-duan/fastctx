//! The command-line program of a release: the package `fastctx-<triple>`
//! for the machine it runs on, a `.tar.gz` (on Windows a `.zip`) with the
//! program and its notices side by side, and on Windows the program's own
//! signature (`fastctx.exe.sig`), which its boot service takes it by: the
//! package's signature is over the archive, never the program.

use std::path::{Path, PathBuf};

use fastctx_types::update::KERNEL_PREFIX;

/// The package this machine's World program comes in.
pub fn target() -> String {
    format!("{KERNEL_PREFIX}{}", triple())
}

/// The release's command-line target for this platform. Windows on Arm uses
/// the published x64 program; Linux's static programs keep their `gnu` name.
pub fn triple() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("windows", "x86_64" | "aarch64") => "x86_64-pc-windows-msvc",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        _ => "unknown",
    }
}

/// The program's name in its archive.
fn program() -> &'static str {
    if cfg!(windows) { "fastctx.exe" } else { "fastctx" }
}

/// The name of the program's own signature beside it.
fn signature_name() -> String {
    format!("{}.sig", program())
}

/// Takes the program out of the checked `archive` into `into`, a directory
/// of the caller's, and says where it is; its own signature, when the
/// archive carries one, goes beside it.
pub fn unpack(archive: &Path, into: &Path) -> Result<PathBuf, String> {
    let name = program();
    let out = into.join(name);
    let mut found = false;
    each(archive, &mut |entry, reader| {
        let to = if entry == name {
            found = true;
            out.clone()
        } else if entry == signature_name() {
            into.join(signature_name())
        } else {
            return Ok(());
        };
        let mut written = std::fs::File::create(&to).map_err(|error| format!("{}: {error}", to.display()))?;
        std::io::copy(reader, &mut written).map_err(|error| format!("unpacking {}: {error}", to.display()))?;
        written.sync_all().map_err(|error| format!("{}: {error}", to.display()))
    })?;
    if !found {
        return Err(format!("{} has no {name}", archive.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755)).map_err(|error| error.to_string())?;
    }
    Ok(out)
}

/// The program's own signature `archive` carries, if it carries one.
pub fn signature(archive: &Path) -> Result<Option<String>, String> {
    let mut said = None;
    each(archive, &mut |entry, reader| {
        if entry == signature_name() {
            let mut text = String::new();
            std::io::Read::read_to_string(reader, &mut text).map_err(|error| error.to_string())?;
            said = Some(text);
        }
        Ok(())
    })?;
    Ok(said)
}

/// Whether the program `archive` carries is, byte for byte, the file at
/// `executable`: where a release signs only its packages, a checked package
/// vouches for a program this way.
pub fn carries(archive: &Path, executable: &Path) -> Result<bool, String> {
    let name = program();
    let mut file = std::fs::File::open(executable).map_err(|error| format!("{}: {error}", executable.display()))?;
    let mut same = None;
    each(archive, &mut |entry, reader| {
        if entry == name {
            same =
                Some(equal(reader, &mut file).map_err(|error| format!("comparing {}: {error}", executable.display()))?);
        }
        Ok(())
    })?;
    same.ok_or_else(|| format!("{} has no {name}", archive.display()))
}

fn equal(left: &mut dyn std::io::Read, right: &mut dyn std::io::Read) -> std::io::Result<bool> {
    let (mut a, mut b) = (vec![0; 64 * 1024], vec![0; 64 * 1024]);
    loop {
        let (n, m) = (fill(left, &mut a)?, fill(right, &mut b)?);
        if a[..n] != b[..m] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

/// Reads until `buffer` is full or the input ends.
fn fill(input: &mut dyn std::io::Read, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match input.read(&mut buffer[filled..])? {
            0 => break,
            count => filled += count,
        }
    }
    Ok(filled)
}

/// Hands each file at the top of `archive`, a `.zip` or a `.tar.gz`, to
/// `take` by its name.
fn each(
    archive: &Path,
    take: &mut dyn FnMut(&str, &mut dyn std::io::Read) -> Result<(), String>,
) -> Result<(), String> {
    let reading = |error: &dyn std::fmt::Display| format!("reading {}: {error}", archive.display());
    let mut file = std::fs::File::open(archive).map_err(|error| format!("{}: {error}", archive.display()))?;
    let mut magic = [0; 4];
    let zipped =
        std::io::Read::read(&mut file, &mut magic).map_err(|error| error.to_string())? == 4 && magic == *b"PK\x03\x04";
    std::io::Seek::rewind(&mut file).map_err(|error| error.to_string())?;
    if zipped {
        let mut entries = zip::ZipArchive::new(file).map_err(|error| reading(&error))?;
        for index in 0..entries.len() {
            let mut entry = entries.by_index(index).map_err(|error| reading(&error))?;
            let name = entry.name().to_string();
            if !name.contains('/') {
                take(&name, &mut entry)?;
            }
        }
        return Ok(());
    }
    let mut entries = tar::Archive::new(flate2::read::GzDecoder::new(file));
    for entry in entries.entries().map_err(|error| reading(&error))? {
        let mut entry = entry.map_err(|error| reading(&error))?;
        let path = entry.path().map_err(|error| error.to_string())?.into_owned();
        if path.components().count() == 1
            && let Some(name) = path.file_name().and_then(|name| name.to_str()).map(str::to_string)
        {
            take(&name, &mut entry)?;
        }
    }
    Ok(())
}
