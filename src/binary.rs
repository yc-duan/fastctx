//! Magic-byte recognition for common binary formats shared by read and grep.

/// Returns the contract file type used in binary errors, declining to guess when no signature matches.
pub(crate) fn detect_binary_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"PK\x03\x04") {
        return Some("a ZIP archive");
    }
    if bytes.starts_with(b"\x1F\x8B") {
        return Some("gzip data");
    }
    if bytes.starts_with(b"\x37\x7A\xBC\xAF\x27\x1C") {
        return Some("7-Zip data");
    }
    if bytes.starts_with(b"\x28\xB5\x2F\xFD") {
        return Some("Zstandard data");
    }
    if bytes.starts_with(b"\x7FELF") {
        return Some("an ELF executable");
    }
    if bytes.starts_with(b"MZ") {
        return Some("a Windows executable");
    }
    if [
        b"\xFE\xED\xFA\xCE".as_slice(),
        b"\xFE\xED\xFA\xCF".as_slice(),
        b"\xCE\xFA\xED\xFE".as_slice(),
        b"\xCF\xFA\xED\xFE".as_slice(),
    ]
    .iter()
    .any(|magic| bytes.starts_with(magic))
    {
        return Some("a Mach-O executable");
    }
    if bytes.starts_with(b"SQLite format 3\0") {
        return Some("a SQLite database");
    }
    if bytes.starts_with(b"\0asm") {
        return Some("a WebAssembly module");
    }
    if bytes.get(257..262) == Some(b"ustar") {
        return Some("a tar archive");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::detect_binary_type;

    #[test]
    fn recognizes_each_contract_signature_without_extension_help() {
        let cases: &[(&[u8], &str)] = &[
            (b"PK\x03\x04", "a ZIP archive"),
            (b"\x1F\x8B", "gzip data"),
            (b"\x37\x7A\xBC\xAF\x27\x1C", "7-Zip data"),
            (b"\x28\xB5\x2F\xFD", "Zstandard data"),
            (b"\x7FELF", "an ELF executable"),
            (b"MZ", "a Windows executable"),
            (b"\xFE\xED\xFA\xCE", "a Mach-O executable"),
            (b"\xFE\xED\xFA\xCF", "a Mach-O executable"),
            (b"\xCE\xFA\xED\xFE", "a Mach-O executable"),
            (b"\xCF\xFA\xED\xFE", "a Mach-O executable"),
            (b"SQLite format 3\0", "a SQLite database"),
            (b"\0asm", "a WebAssembly module"),
        ];
        for (bytes, expected) in cases {
            assert_eq!(detect_binary_type(bytes), Some(*expected));
        }

        let mut tar = vec![0_u8; 262];
        tar[257..262].copy_from_slice(b"ustar");
        assert_eq!(detect_binary_type(&tar), Some("a tar archive"));
        assert_eq!(detect_binary_type(b"ordinary text"), None);
    }
}
