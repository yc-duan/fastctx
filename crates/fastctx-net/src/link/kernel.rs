//! Release information comes from the root, even on a relayed session.

use fastctx_types::kernel::KernelRelease;

use super::Link;
use crate::trust::CertPin;
use crate::wire::{Question, Told};

impl Link {
    /// Gets a complete release and verifies its public certificate against
    /// the root identity in this membership before callers compute an SPKI pin.
    pub async fn kernel_release(&self, version: &str) -> Result<(KernelRelease, Vec<u8>), String> {
        match self.inner.ask(Question::KernelRelease { version: version.to_owned() }).await? {
            Told::KernelRelease { release, certificate } => {
                if release.version != version {
                    return Err(format!("the hub answered for fastctx {}, requested {version}", release.version));
                }
                if CertPin::of(&certificate) != self.membership().cert {
                    return Err(
                        "the release certificate is not the root hub certificate pinned by this membership".into()
                    );
                }
                Ok((release, certificate))
            }
            _ => Err("the hub answered something other than a kernel release".into()),
        }
    }
}
