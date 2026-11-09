//! The frozen field framing does not depend on serde's JSON representation.

use sha2::{Digest as _, Sha256};

pub(super) struct Encoder(Sha256);

impl Encoder {
    pub(super) fn new(domain: &str) -> Self {
        let mut this = Self(Sha256::new());
        this.field(b"fastctx.conversation-identity.v1");
        this.text(domain);
        this
    }

    pub(super) fn field(&mut self, bytes: &[u8]) {
        self.0.update((bytes.len() as u64).to_be_bytes());
        self.0.update(bytes);
    }

    pub(super) fn text(&mut self, value: &str) {
        self.field(value.as_bytes());
    }

    pub(super) fn index(&mut self, value: u64) {
        self.field(&value.to_be_bytes());
    }

    pub(super) fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}
