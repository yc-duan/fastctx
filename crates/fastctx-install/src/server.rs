//! A short POSIX bootstrap command for a machine invited by a GUI.
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
