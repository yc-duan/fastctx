//! Verified hand-placed archives take precedence over the running Linux kernel.

mod files;
pub(crate) mod http;
#[cfg(target_os = "linux")]
mod own;

use std::fs::File;
use std::io::{self, Read as _};
use std::path::Path;

use fastctx_types::kernel::{AARCH64_ARCHIVE, HELD_ARCHIVES, KernelRelease, SUMS, X86_64_ARCHIVE, release_version};
use sha2::{Digest as _, Sha256};

pub(crate) struct Releases {
    files: files::Files,
    pub certificate: Vec<u8>,
    #[cfg(target_os = "linux")]
    own: own::Own,
}

impl Releases {
    pub fn new(directory: &Path, certificate: Vec<u8>) -> io::Result<Self> {
        Ok(Self {
            files: files::Files::new(directory)?,
            certificate,
            #[cfg(target_os = "linux")]
            own: own::Own::new()?,
        })
    }

    /// One published file, for a download. Placed files are not hashed here:
    /// the line that downloads it carries the digest and checks it, and
    /// hashing both archives for every unauthenticated request would let
    /// anyone keep the hub's processors busy with a few bytes each. The
    /// running kernel's own archive is packed at most once per process.
    pub fn file(&self, version: &str, name: &str) -> io::Result<File> {
        match self.files.open(version, name) {
            Ok(file) => Ok(file),
            #[cfg(target_os = "linux")]
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    && version == env!("CARGO_PKG_VERSION")
                    && (Some(name) == own::name() || name == SUMS) =>
            {
                // A line made before this hub restarted still downloads: the
                // archive is packed at most once per process, whoever asks.
                self.own.digest(&self.files.root).map_err(io::Error::other)?;
                self.own.file(name == SUMS)
            }
            Err(error) => Err(error),
        }
    }

    /// An archive placed among the releases by hand, and only that.
    pub fn placed(&self, version: &str, name: &str) -> io::Result<File> {
        self.files.open(version, name)
    }

    pub fn ready(&self, version: &str, allow_partial: bool) -> Result<KernelRelease, String> {
        if !release_version(version) {
            return Err("a kernel release version must be major.minor.patch, with at most a pre-release".into());
        }
        self.load(version, allow_partial).map_err(|error| {
            format!(
                "fastctx {version} is not ready on the hub: {error}. Place the required Linux archives and {SUMS} as regular copied files in {}. Prepare them in a staging directory and rename it to v{version} when complete",
                // A canonical Windows path is verbatim, `\\?\C:\…`; people write it without the prefix.
                self.files.root.join("releases").join(format!("v{version}")).display().to_string().trim_start_matches(r"\\?\")
            )
        })
    }

    fn load(&self, version: &str, allow_partial: bool) -> Result<KernelRelease, String> {
        let sums = match self.files.open(version, SUMS) {
            Ok(mut file) => {
                let mut text = String::new();
                file.read_to_string(&mut text).map_err(|error| format!("reading {SUMS}: {error}"))?;
                Some(text)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("{SUMS}: {error}")),
        };
        let x86_64_sha256 = self.archive(version, X86_64_ARCHIVE, sums.as_deref())?;
        let aarch64_sha256 = self.archive(version, AARCH64_ARCHIVE, sums.as_deref())?;
        if !allow_partial && (x86_64_sha256.is_none() || aarch64_sha256.is_none()) {
            return Err("this caller requires both x86_64 and aarch64 archives".into());
        }
        let mut release = KernelRelease {
            version: version.to_owned(),
            x86_64_sha256,
            aarch64_sha256,
            windows_x86_64_sha256: None,
            darwin_aarch64_sha256: None,
            darwin_x86_64_sha256: None,
        };
        // The Windows and Mac archives are never required: placed here, or
        // else held among the update packages (`crate::updates::held_digest`).
        for (archive, _) in HELD_ARCHIVES {
            let digest = self.archive(version, archive, sums.as_deref())?;
            *release.held(archive).expect("a held archive") = digest;
        }
        Ok(release)
    }

    fn archive(&self, version: &str, name: &str, sums: Option<&str>) -> Result<Option<String>, String> {
        match self.files.open(version, name) {
            Ok(mut file) => {
                let digest = checksum(sums.ok_or_else(|| format!("missing {SUMS}"))?, name)?;
                verify(&mut file, name, &digest)?;
                Ok(Some(digest))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                #[cfg(target_os = "linux")]
                if version == env!("CARGO_PKG_VERSION") && Some(name) == own::name() {
                    return self.own.digest(&self.files.root).map(Some);
                }
                Ok(None)
            }
            Err(error) => Err(format!("{name}: {error}")),
        }
    }
}

fn checksum(text: &str, name: &str) -> Result<String, String> {
    let mut found = None;
    for line in text.lines().filter(|line| !line.is_empty()) {
        let bytes = line.as_bytes();
        if bytes.len() < 67
            || !bytes[..64].iter().all(u8::is_ascii_hexdigit)
            || bytes[64] != b' '
            || !matches!(bytes[65], b' ' | b'*')
        {
            return Err(format!("invalid {SUMS} line"));
        }
        if &line[66..] == name {
            if found.is_some() {
                return Err(format!("{SUMS} repeats {name}"));
            }
            found = Some(line[..64].to_ascii_lowercase());
        }
    }
    found.ok_or_else(|| format!("{SUMS} has no digest for {name}"))
}

fn verify(file: &mut File, name: &str, expected: &str) -> Result<(), String> {
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| format!("reading {name}: {error}"))?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let actual = format!("{:x}", hash.finalize());
    if actual != expected {
        return Err(format!("{name} does not match its {SUMS} digest"));
    }
    Ok(())
}
