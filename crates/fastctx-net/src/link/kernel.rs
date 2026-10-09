//! Release information comes from the hub, through a relay as well.

use fastctx_types::kernel::KernelRelease;

use super::Link;
use crate::ids::CertPin;
use crate::wire::{Question, Told};

impl Link {
    /// Gets a complete release and checks its certificate against the one
    /// this membership pins, before callers compute an SPKI pin from it.
    pub async fn kernel_release(&self, version: &str) -> Result<(KernelRelease, Vec<u8>), String> {
        match self.inner.ask(Question::KernelRelease { version: version.to_owned() }).await? {
            Told::KernelRelease { release, certificate } => {
                if release.version != version {
                    return Err(format!("the hub answered for fastctx {}, requested {version}", release.version));
                }
                if CertPin::of(&certificate) != self.membership().cert {
                    return Err(
                        "the release certificate is not the hub certificate this membership pins".into()
                    );
                }
                Ok((release, certificate))
            }
            _ => Err("the hub answered something other than a kernel release".into()),
        }
    }
}
