//! Published bytes are an external contract, including the absence of an
//! ending newline. Hashes were extracted from the corresponding git tags.

use sha2::{Digest, Sha256};

#[test]
fn published_marker_bodies_are_byte_frozen() {
    let contracts: [(&[u8], &str); 8] = [
        (
            include_bytes!("../src/legacy_guidance/v010.txt"),
            "28cf0e942484af645ecf53c9c5ee8baffe1f0adca28aebcffd4ae1fc0a8ba07d",
        ),
        (
            include_bytes!("../src/legacy_guidance/v010-shell.txt"),
            "5b144d53d0fed3bdebfe69db93b8eb51cb49df931f8c40145a9a76bc36001042",
        ),
        (
            include_bytes!("../src/legacy_guidance/v022.txt"),
            "17da1a91024e98008f7979937e9c6b423c8b77556854082fc985922dfbf248a3",
        ),
        (
            include_bytes!("../src/legacy_guidance/v022-shell.txt"),
            "cf0ac5d64b9b9d615617a2d390a41625007bb0cf3ef1b968596585b8376f5b19",
        ),
        (
            include_bytes!("../src/legacy_guidance/v024.txt"),
            "5aab7b9a170f157b9835611a5799e560722160569fc52f91ea3c6487b6a1e51c",
        ),
        (
            include_bytes!("../src/legacy_guidance/v024-shell.txt"),
            "63ccd0c127b991171b41caea9124827bbad4ac09bf659f505d42f19bd14040d1",
        ),
        (
            include_bytes!("../src/legacy_guidance/v025.txt"),
            "1a3b5e295d0118800eb5812f7f3357f7bf298aa315a1bf3e4d13654666ab0c00",
        ),
        (
            include_bytes!("../src/legacy_guidance/v025-shell.txt"),
            "6a0bdd0cc6595060ca2e83302d0982236d16abe08c49921eb666f5924cbb1514",
        ),
    ];
    for (bytes, expected) in contracts {
        assert_eq!(format!("{:x}", Sha256::digest(bytes)), expected);
    }
}
