//! A short bootstrap command for a machine invited by a GUI: a POSIX line for
//! a Linux server, a cmd line for a Windows computer.
//! The invitation is opaque here; only the installed kernel interprets it.

use anyhow::{bail, ensure};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest as _, Sha256};

#[cfg(target_os = "linux")]
mod placement;
#[cfg(target_os = "linux")]
pub use placement::place;

/// A Linux release's archive digests, as recorded in SHA256SUMS.
#[derive(Clone, Copy, Debug)]
pub struct Kernel<'a> {
    pub version: &'a str,
    pub x86_64_sha256: Option<&'a str>,
    pub aarch64_sha256: Option<&'a str>,
}

/// A release directory containing the platform archive. A hub must supply its
/// SHA-256 over the certificate's DER SubjectPublicKeyInfo, encoded as base64.
/// This is curl's public-key pin, separate from the opaque invitation.
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
/// A download that breaks goes on from where it stopped, again and again,
/// waiting twice as long each time up to a minute (`world-network.md`, 断开不是
/// 结束); a source that cannot resume is read again from the start.
/// Requires a POSIX shell, uname, mktemp, curl, tar and sha256sum; no Python.
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
            (
                release_directory.trim_end_matches('/').to_owned(),
                format!(" -k --pinnedpubkey {}", quote(&format!("sha256//{spki_sha256}"))),
            )
        }
    };
    Ok(format!(
        "(set -e;a=$(uname -m);case $a in x86_64) {};;aarch64) {};;*) echo Unsupported:$a >&2;exit 1;;esac;d=$(mktemp -d);trap 'rm -rf \"$d\"' EXIT;cd \"$d\";w=1;until curl --proto '=https' -fL -C - -o a{pin} {}/fastctx-$a-unknown-linux-gnu.tar.gz;do r=$?;[ $r = 33 ]&&rm -f a;echo \"download stopped (curl $r); again in $w s\" >&2;sleep $w;w=$((w*2));[ $w -gt 60 ]&&w=60;done;echo \"$h  a\"|sha256sum -c -;tar xzf a;./fastctx server install {})",
        architecture(kernel.x86_64_sha256, kernel.version),
        architecture(kernel.aarch64_sha256, kernel.version),
        quote(&directory),
        quote(invitation),
    ))
}

/// Builds the one line for a Windows computer, pasted into an
/// administrator's Command Prompt or PowerShell: in a working folder of its
/// own, it downloads the x64 archive (which Windows on Arm runs too), checks
/// its SHA-256, unpacks it and runs `fastctx server install`. It needs only
/// what Windows 10 1803 and later carry: cmd, curl, certutil and tar. Its one
/// pair of quotes is cmd's own, so either prompt passes it on as it is.
///
/// From a hub it does not pin the hub's key as the Linux line does: the curl
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
    Ok(format!(
        "cmd /c \"rd /s /q {folder} 2>nul & mkdir {folder} && cd /d {folder} && curl.exe --proto =https -fL --retry 5 --retry-delay 3 -o fastctx.zip{pin} {directory}/{archive} && certutil -hashfile fastctx.zip SHA256 | findstr /i /x {hash} >nul && tar -xf fastctx.zip && fastctx.exe server install {invitation}\"",
        archive = fastctx_types::kernel::WINDOWS_X86_64_ARCHIVE,
        hash = sha256.to_ascii_lowercase(),
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
