//! An item named by a name that may be longer than an item's identity
//! holds: the name itself when it fits, otherwise `#` and a digest of it,
//! the whole name in the record's body (OO25, OO26, AR03).

use sha2::{Digest as _, Sha256};

/// The item a record named `name` is kept by.
pub fn item_of(name: &str) -> Vec<u8> {
    if name.len() <= crate::record::KEY_BYTES && !name.starts_with('#') {
        return name.as_bytes().to_vec();
    }
    let digest: String = Sha256::digest(name.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
    format!("#{}", &digest[..crate::record::KEY_BYTES - 1]).into_bytes()
}
