//! Only complete, checksum-matching Linux releases are offered to installers.

mod files;
pub(crate) mod http;

use std::fs::File;
use std::io::{self, Read as _};
use std::path::Path;

use fastctx_types::kernel::{AARCH64_ARCHIVE, KernelRelease, SUMS, X86_64_ARCHIVE, stable_version};
use sha2::{Digest as _, Sha256};

pub(crate) struct Releases {
    files: files::Files,
    pub certificate: Vec<u8>,
}

impl Releases {
    pub fn new(directory: &Path, certificate: Vec<u8>) -> io::Result<Self> {
        Ok(Self { files: files::Files::new(directory)?, certificate })
    }

    /// One published file, for a download. Nothing is hashed here: the line
    /// that downloads it carries the digest and checks it, and hashing both
    /// archives for every unauthenticated request would let anyone keep the
    /// hub's processors busy with a few bytes each.
    pub fn file(&self, version: &str, name: &str) -> io::Result<File> {
        self.files.open(version, name)
    }

    pub fn ready(&self, version: &str) -> Result<KernelRelease, String> {
        if !stable_version(version) {
            return Err("a kernel release version must be stable major.minor.patch".into());
        }
        self.load(version).map_err(|error| {
            format!(
                "fastctx {version} is not ready on the hub: {error}. Place {X86_64_ARCHIVE}, {AARCH64_ARCHIVE}, and {SUMS} as regular copied files in {}. Prepare all three in a staging directory and rename it to v{version} when complete",
                // A canonical Windows path is verbatim, `\\?\C:\…`; people write it without the prefix.
                self.files.root.join("releases").join(format!("v{version}")).display().to_string().trim_start_matches(r"\\?\")
            )
        })
    }

    fn load(&self, version: &str) -> Result<KernelRelease, String> {
        let mut files = Vec::new();
        let mut faults = Vec::new();
        for name in [SUMS, X86_64_ARCHIVE, AARCH64_ARCHIVE] {
            match self.files.open(version, name) {
                Ok(file) => files.push(file),
                Err(error) if error.kind() == io::ErrorKind::NotFound => faults.push(format!("missing {name}")),
                Err(error) => faults.push(format!("{name}: {error}")),
            }
        }
        if !faults.is_empty() {
            return Err(faults.join("; "));
        }
        let [mut sums, mut x86_64, mut aarch64]: [File; 3] =
            files.try_into().expect("the three release files were opened");
        let mut bytes = Vec::new();
        sums.read_to_end(&mut bytes).map_err(|error| error.to_string())?;
        let text = std::str::from_utf8(&bytes).map_err(|error| format!("{SUMS} is not UTF-8: {error}"))?;
        let x86_64_sha256 = checksum(text, X86_64_ARCHIVE)?;
        let aarch64_sha256 = checksum(text, AARCH64_ARCHIVE)?;
        verify(&mut x86_64, X86_64_ARCHIVE, &x86_64_sha256)?;
        verify(&mut aarch64, AARCH64_ARCHIVE, &aarch64_sha256)?;
        Ok(KernelRelease { version: version.to_owned(), x86_64_sha256, aarch64_sha256 })
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
