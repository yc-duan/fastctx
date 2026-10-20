//! A short bootstrap command for a machine invited by a GUI: a POSIX line for
//! a Linux server or Mac, a cmd line for a Windows computer.
//! The invitation is opaque here; only the installed kernel interprets it.

use anyhow::{bail, ensure};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest as _, Sha256};

#[cfg(target_os = "linux")]
mod placement;
#[cfg(target_os = "linux")]
pub use placement::place;

/// A release's archive digests for one system, as recorded in SHA256SUMS:
/// Linux's or the Mac's, by architecture.
#[derive(Clone, Copy, Debug)]
pub struct Kernel<'a> {
    pub version: &'a str,
    pub x86_64_sha256: Option<&'a str>,
    pub aarch64_sha256: Option<&'a str>,
}

/// A release directory containing the platform archive. A hub must supply its
/// SHA-256 over the certificate's DER SubjectPublicKeyInfo, encoded as base64.
/// The fingerprint is validated as issuer metadata. Bootstrap downloads are
/// authenticated by the archive digest carried in the installation line.
#[derive(Clone, Copy, Debug)]
pub enum Source<'a> {
    GitHub,
    Hub { release_directory: &'a str, spki_sha256: &'a str },
}

/// Computes curl's base64 SHA-256 pin from a complete DER certificate.
/// The invitation issuer already owns this public certificate; this function
/// extracts its SubjectPublicKeyInfo without changing the World's trust rules.
pub fn certificate_pin(certificate_der: &[u8]) -> anyhow::Result<String> {
    let (remaining, certificate) = x509_parser::parse_x509_certificate(certificate_der)
        .map_err(|error| anyhow::anyhow!("reading the hub's public certificate: {error}"))?;
    ensure!(remaining.is_empty(), "extra bytes follow the hub's DER certificate");
    Ok(STANDARD.encode(Sha256::digest(certificate.public_key().raw)))
}

/// Builds one physical shell line: select the host architecture, download and
/// verify the complete archive, unpack it, then run `fastctx server install`.
/// Hub downloads use `-k`: the line's archive digest authenticates every byte
/// before extraction, without depending on curl's public-key pin support.
/// Interrupted transfers resume with bounded backoff; permanent errors stop.
/// Requires a POSIX shell, uname, curl, tar and sha256sum; no Python.
/// The kernel owns durable installation, enrollment and service registration.
pub fn command(kernel: Kernel<'_>, invitation: &str, source: Source<'_>) -> anyhow::Result<String> {
    ensure!(
        fastctx_types::kernel::release_version(kernel.version),
        "the release version must be major.minor.patch, with at most a pre-release"
    );
    ensure!(
        !invitation.is_empty() && !invitation.contains('\0') && !invitation.chars().any(char::is_whitespace),
        "the invitation must be one nonempty word"
    );
    for value in [kernel.x86_64_sha256, kernel.aarch64_sha256].into_iter().flatten() {
        digest(value)?;
    }
    let (directory, pin) = match source {
        Source::GitHub => {
            (format!("https://github.com/yc-duan/fastctx/releases/download/v{}", kernel.version), String::new())
        }
        Source::Hub { release_directory, spki_sha256 } => {
            ensure!(release_directory.starts_with("https://"), "a hub release directory must use HTTPS");
            ensure!(!release_directory.contains(['\0', '\n', '\r', '?', '#']), "invalid hub release directory");
            let bytes = STANDARD.decode(spki_sha256)?;
            ensure!(
                bytes.len() == 32 && STANDARD.encode(&bytes) == spki_sha256,
                "SPKI SHA-256 must be a canonical base64-encoded 32-byte digest"
            );
            (release_directory.trim_end_matches('/').to_owned(), " -k".to_owned())
        }
    };
    let download = unix_download(kernel.version, "unknown-linux-gnu", &directory, &pin, "sha256sum");
    Ok(format!(
        "(set -e;a=$(uname -m);case $a in x86_64) {};;aarch64) {};;*) echo Unsupported:$a >&2;exit 1;;esac;{download};tar xzf a;./fastctx server install {};cd /;rm -rf \"$d\")",
        architecture(kernel.x86_64_sha256, kernel.version),
        architecture(kernel.aarch64_sha256, kernel.version),
        quote(invitation),
    ))
}

/// Builds the one line for a Mac, shaped as the Linux one: Apple silicon or
/// Intel by `uname -m`, a download that goes on from where it broke, checked
/// with `shasum`, which every Mac has. From a hub it trusts the digest it
/// carries, as the Linux and Windows lines do, without a curl key pin.
/// Run as root it makes a node that starts at boot; run as a person, theirs.
pub fn mac_command(kernel: Kernel<'_>, invitation: &str, source: Source<'_>) -> anyhow::Result<String> {
    ensure!(
        fastctx_types::kernel::release_version(kernel.version),
        "the release version must be major.minor.patch, with at most a pre-release"
    );
    ensure!(
        !invitation.is_empty() && !invitation.contains('\0') && !invitation.chars().any(char::is_whitespace),
        "the invitation must be one nonempty word"
    );
    for value in [kernel.x86_64_sha256, kernel.aarch64_sha256].into_iter().flatten() {
        digest(value)?;
    }
    ensure!(
        kernel.x86_64_sha256.is_some() || kernel.aarch64_sha256.is_some(),
        "fastctx {} for Macs is unavailable from this source; make the line again in a few minutes, or place {} or {} with {} on the hub",
        kernel.version,
        fastctx_types::kernel::DARWIN_AARCH64_ARCHIVE,
        fastctx_types::kernel::DARWIN_X86_64_ARCHIVE,
        fastctx_types::kernel::SUMS
    );
    let (directory, insecure) = match source {
        Source::GitHub => (format!("https://github.com/yc-duan/fastctx/releases/download/v{}", kernel.version), ""),
        Source::Hub { release_directory, .. } => {
            ensure!(release_directory.starts_with("https://"), "a hub release directory must use HTTPS");
            ensure!(!release_directory.contains(['\0', '\n', '\r', '?', '#']), "invalid hub release directory");
            (release_directory.trim_end_matches('/').to_owned(), " -k")
        }
    };
    let download = unix_download(kernel.version, "apple-darwin", &directory, insecure, "shasum -a 256");
    Ok(format!(
        "(set -e;case $(uname -m) in arm64) a=aarch64;{};;x86_64) a=x86_64;{};;*) echo Unsupported:$(uname -m) >&2;exit 1;;esac;{download};tar xzf a;./fastctx server install {};cd /;rm -rf \"$d\")",
        mac_architecture(kernel.aarch64_sha256, kernel.version),
        mac_architecture(kernel.x86_64_sha256, kernel.version),
        quote(invitation),
    ))
}

/// A failed attempt stays in the caller's home, apart from other versions
/// and platforms; a root shell a remote manager started may have no home,
/// and then it stays in `/var/tmp`, which a restart keeps. A complete archive is reused; an incorrect one is resumed
/// and checked, then fetched afresh, just as in the Windows line.
fn unix_download(version: &str, target: &str, directory: &str, options: &str, checksum: &str) -> String {
    include_str!("server/download.sh")
        .replace("@VERSION@", version)
        .replace("@TARGET@", target)
        .replace("@CHECKSUM@", checksum)
        .replace("@OPTIONS@", options)
        .replace("@URL@", &quote(directory))
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ")
}

fn mac_architecture(digest: Option<&str>, version: &str) -> String {
    match digest {
        Some(digest) => format!("h={}", digest.to_ascii_lowercase()),
        None => format!(
            "echo \"fastctx {version} for $a Macs is unavailable from this source; make the line again in a few minutes, or place the archive with SHA256SUMS on the hub.\" >&2;exit 1"
        ),
    }
}

/// Builds the one line for a Windows computer, pasted into an
/// administrator's Command Prompt or PowerShell: in a working folder of its
/// own, it downloads the x64 archive (which Windows on Arm runs too), checks
/// its SHA-256, unpacks it and runs `fastctx server install`. It needs only
/// what Windows 10 1803 and later carry: cmd, curl, certutil and tar. Its one
/// pair of quotes is cmd's own, so either prompt passes it on as it is.
///
/// From a hub, like the Unix lines, it authenticates the archive digest: curl
/// of many Windows releases cannot (Windows Server 2022's answers that the
/// option is not built in). The digest the line carries is what holds: an
/// archive that is not the one meant is never unpacked, whoever served it,
/// and the code goes nowhere until the program uses it over its own pinned
/// connection.
pub fn windows_command(
    version: &str,
    sha256: Option<&str>,
    invitation: &str,
    source: Source<'_>,
) -> anyhow::Result<String> {
    ensure!(
        fastctx_types::kernel::release_version(version),
        "the release version must be major.minor.patch, with at most a pre-release"
    );
    // A word cmd reads as it is: nothing it expands, joins or redirects by.
    ensure!(
        !invitation.is_empty() && invitation.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "a Windows line carries an invitation of letters, digits and hyphens only"
    );
    let sha256 = sha256.ok_or_else(|| {
        anyhow::anyhow!(
            "fastctx {version} for Windows is not on this hub yet; it is being brought in, so make the line again in a few minutes, or place {} with {} on the hub",
            fastctx_types::kernel::WINDOWS_X86_64_ARCHIVE,
            fastctx_types::kernel::SUMS
        )
    })?;
    digest(sha256)?;
    let (directory, pin) = match source {
        Source::GitHub => (format!("https://github.com/yc-duan/fastctx/releases/download/v{version}"), String::new()),
        Source::Hub { release_directory, .. } => {
            ensure!(release_directory.starts_with("https://"), "a hub release directory must use HTTPS");
            ensure!(
                release_directory.bytes().all(|byte| byte.is_ascii_alphanumeric() || b":/.-[]_".contains(&byte)),
                "invalid hub release directory for a Windows line"
            );
            (release_directory.trim_end_matches('/').to_owned(), " -k".to_owned())
        }
    };
    let folder = r"%ProgramData%\fastctx-install";
    // Windows' own tools, by where Windows keeps them: a curl or a tar of
    // another kind earlier on the PATH, as Git's, does not take their place.
    let system = r"%SystemRoot%\System32";
    // Only the download's arguments are kept in a variable: a program named
    // by a delayed expansion is taken whole, arguments and all, as its path.
    let arguments = format!(
        "--proto =https -fL --retry 5 --retry-delay 3 -o fastctx.zip{pin} {directory}/{archive}",
        archive = fastctx_types::kernel::WINDOWS_X86_64_ARCHIVE
    );
    let download = format!(r"{system}\curl.exe !d!");
    // An archive already whole is kept; else the download goes on from what
    // came, three times, and then starts afresh, each followed by the check.
    // Delayed expansion (`/v:on`) writes the download's arguments once; the
    // check is written out each time, since what runs in a pipe does not
    // expand it. What is downloaded stays for the line run again until it
    // installed. Every way the line stops ends with a line saying why.
    let check = format!(
        r"({system}\certutil.exe -hashfile fastctx.zip SHA256 | {system}\findstr.exe /i /x {} >nul)",
        sha256.to_ascii_lowercase()
    );
    Ok(format!(
        "cmd /v:on /c \"set d={arguments}&& \
         (where /q {system}:curl.exe && where /q {system}:tar.exe || \
         (echo fastctx: this Windows has no curl or tar; update it to Windows 10 1803 or later& exit /b 1)) && \
         (mkdir {folder} 2>nul & cd /d {folder}) && \
         ({check} || (({download} -C - || {download} -C - || {download} -C -) & {check}) || \
         (del fastctx.zip 2>nul & {download} & {check}) || \
         (echo fastctx: downloading the release failed or brought another file; run the line again to continue& exit /b 1)) && \
         {system}\\tar.exe -xf fastctx.zip && .\\fastctx.exe server install {invitation} && \
         (cd /d %ProgramData% & rd /s /q fastctx-install 2>nul & ver >nul)\""
    ))
}

fn architecture(digest: Option<&str>, version: &str) -> String {
    match digest {
        Some(digest) => format!("h={}", digest.to_ascii_lowercase()),
        None => format!(
            "echo \"fastctx {version} for $a is unavailable from this source; get the $a Linux archive from GitHub Releases or place it with SHA256SUMS on the hub.\" >&2;exit 1"
        ),
    }
}

fn digest(value: &str) -> anyhow::Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("SHA-256 must contain exactly 64 hexadecimal digits");
    }
    Ok(())
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
