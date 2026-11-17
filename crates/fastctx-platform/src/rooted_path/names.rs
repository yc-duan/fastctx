//! Exact native components and deterministic alternatives for unrepresentable names.
use fastctx_types::sync::{
    identity::{Digest, ObjectId},
    manifest::PathName,
};
use std::{
    ffi::{OsStr, OsString},
    io,
    path::{Component, Path},
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(OsString);
impl Name {
    /// A single component. Never a drive, UNC path, stream, dot or parent path.
    pub fn new(value: impl AsRef<OsStr>) -> io::Result<Self> {
        let value = value.as_ref();
        let mut parts = Path::new(value).components();
        if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() {
            return Err(invalid());
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let bytes = value.as_bytes();
            if bytes.is_empty() || bytes.len() > 255 || bytes.contains(&0) || bytes.contains(&b'/') {
                return Err(invalid());
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            let units = value.encode_wide().collect::<Vec<_>>();
            validate_windows(&units)?;
        }
        Ok(Self(value.to_owned()))
    }
    /// Shared with portable skills, including case, full-width and canonical
    /// Unicode collisions. Original native bytes are never rewritten in place.
    pub fn key(&self) -> Option<String> {
        self.0.to_str().map(fastctx_types::skill_files::path_key)
    }
    pub fn could_alias(&self, other: &Self) -> bool {
        match (self.key(), other.key()) {
            (Some(left), Some(right)) => left == right,
            _ => native_alias(self, other),
        }
    }
    pub fn as_os_str(&self) -> &OsStr {
        &self.0
    }
    pub fn from_source(source: &PathName) -> io::Result<Self> {
        let native = match source {
            PathName::Utf8(value) => OsString::from(value),
            PathName::UnixBytes(value) => unix_name(value)?,
            PathName::WindowsUnits(value) => windows_name(value)?,
        };
        Self::new(native)
    }
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "unrepresentable or unsafe path component")
}

/// Useful before staging a complete Windows directory even on another platform.
pub fn validate_windows(units: &[u16]) -> io::Result<()> {
    if units.is_empty()
        || units.len() > 255
        || units.iter().any(|v| *v < 32 || [34, 42, 47, 58, 60, 62, 63, 92, 124].contains(v))
        || matches!(units.last(), Some(32 | 46))
    {
        return Err(invalid());
    }
    let text = String::from_utf16_lossy(units);
    if text == "." || text == ".." {
        return Err(invalid());
    }
    let stem = text.split('.').next().unwrap_or_default().trim_end_matches(' ').to_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix)
                .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"))
        })
    {
        return Err(invalid());
    }
    Ok(())
}
#[cfg(unix)]
fn unix_name(bytes: &[u8]) -> io::Result<OsString> {
    use std::os::unix::ffi::OsStringExt;
    Ok(OsString::from_vec(bytes.to_vec()))
}
#[cfg(windows)]
fn unix_name(bytes: &[u8]) -> io::Result<OsString> {
    Ok(std::str::from_utf8(bytes).map_err(|_| invalid())?.into())
}
#[cfg(windows)]
fn windows_name(units: &[u16]) -> io::Result<OsString> {
    use std::os::windows::ffi::OsStringExt;
    Ok(OsString::from_wide(units))
}
#[cfg(unix)]
fn windows_name(units: &[u16]) -> io::Result<OsString> {
    Ok(String::from_utf16(units).map_err(|_| invalid())?.into())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameMapping {
    pub original: PathName,
    pub local: Name,
    pub aliased: bool,
}
/// The caller persists the returned mapping in both directions. `collision`
/// comes from the complete directory's name reservation pass, including case
/// and file/directory-prefix collisions. Do not resolve those by overwriting.
/// Skills require exact names and report a blocked materialization instead.
pub fn map_name(
    source: &PathName,
    object: &ObjectId,
    exact: bool,
    collision: bool,
    suffix_digits: usize,
) -> io::Result<NameMapping> {
    if !collision && let Ok(local) = Name::from_source(source) {
        return Ok(NameMapping { original: source.clone(), local, aliased: false });
    }
    if exact {
        return Err(invalid());
    }
    if !(12..=64).contains(&suffix_digits) {
        return Err(invalid());
    }
    let mut bytes = b"fastctx.materialized-name.v1\0".to_vec();
    bytes.extend_from_slice(object.as_str().as_bytes());
    match source {
        PathName::Utf8(value) => {
            bytes.push(0);
            bytes.extend_from_slice(value.as_bytes());
        }
        PathName::UnixBytes(value) => {
            bytes.push(1);
            bytes.extend_from_slice(value);
        }
        PathName::WindowsUnits(value) => {
            bytes.push(2);
            for unit in value {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
        }
    }
    let digest = Digest::of(&bytes);
    let local = Name::new(format!("asset-{}", &digest.as_str()[..suffix_digits]))?;
    Ok(NameMapping { original: source.clone(), local, aliased: true })
}

#[cfg(unix)]
fn native_alias(left: &Name, right: &Name) -> bool {
    left == right
}
#[cfg(windows)]
fn native_alias(left: &Name, right: &Name) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
    let left: Vec<u16> = left.as_os_str().encode_wide().collect();
    let right: Vec<u16> = right.as_os_str().encode_wide().collect();
    // SAFETY: explicit lengths cover the native code units, including names
    // that cannot be represented losslessly as a Rust UTF-8 string.
    let result =
        unsafe { CompareStringOrdinal(left.as_ptr(), left.len() as i32, right.as_ptr(), right.len() as i32, 1) };
    result == 0 || result == CSTR_EQUAL
}
