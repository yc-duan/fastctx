//! Release information comes from the hub, whichever way this machine
//! reaches it.

use fastctx_types::kernel::KernelRelease;

use super::Link;
use crate::ids::CertPin;
use crate::wire::{Question, Told};

impl Link {
    /// Gets available archives and checks their certificate against the one
    /// this membership pins, before callers compute an SPKI pin from it.
    pub async fn kernel_release(&self, version: &str, wanted: &[&str]) -> Result<(KernelRelease, Vec<u8>), String> {
        let wanted = wanted.iter().map(|archive| (*archive).to_owned()).collect();
        let question = Question::KernelRelease { version: version.to_owned(), allow_partial: true, wanted };
        match self.inner.ask(question).await? {
            Told::KernelRelease { release, certificate } => {
                if release.version != version {
                    return Err(format!("the hub answered for fastctx {}, requested {version}", release.version));
                }
                if CertPin::of(&certificate) != self.membership().cert {
                    return Err("the release certificate is not the hub certificate this membership pins".into());
                }
                Ok((release, certificate))
            }
            _ => Err("the hub answered something other than a kernel release".into()),
        }
    }
}
