//! Restores the machine's persisted environment for the commands the shell tools execute.
//!
//! A stdio MCP server does not receive the environment its user configured. Codex clears the
//! child environment and re-adds a fixed list of about twenty names, so variables like
//! `JAVA_HOME`, `GOPATH`, or `CUDA_PATH` never reach this process and therefore never reach a
//! command the model runs — while Codex's own shell tool, on the same machine, inherits all of
//! them. This module closes that asymmetry by reading the environment the operating system
//! persists for the user and placing it *underneath* whatever the host did provide.
//!
//! The result is used only for commands the user runs. FastCtx keeps resolving its own state
//! directories, endpoint identity, and output budgets from the unmodified host environment: a
//! machine that persists `HOME` must not silently relocate `~/.fastctx` on upgrade.

use crate::session::SessionEnvironment;
use std::ffi::{OsStr, OsString};

/// Name of the search-path variable rebuilt by union instead of replacement.
const SEARCH_PATH: &str = "PATH";

/// Opt-out for anyone who wants commands to see the host's exact environment and nothing else.
const INHERIT_ENV: &str = "FASTCTX_INHERIT_ENVIRONMENT";

#[cfg(windows)]
const PATH_LIST_SEPARATOR: char = ';';
#[cfg(not(windows))]
const PATH_LIST_SEPARATOR: char = ':';

/// Builds the environment a user command should see for one session.
///
/// Host-provided values always win: they are either live session state or literals the user wrote
/// into the host's own server configuration, and both are more specific than a persisted default.
/// `PATH` is the one exception — it is a union, because dropping either side loses real tools.
pub(crate) fn command_environment(host: &SessionEnvironment) -> SessionEnvironment {
    let persisted = persisted_environment(host.variables());
    if persisted.is_empty() {
        return host.clone();
    }
    let merged = overlay(persisted, host.variables());
    // Checked against the merged view so the opt-out works from the machine's own environment
    // settings, which is where someone who wants it turned off will reach for it first.
    if !inheritance_enabled(&merged) {
        return host.clone();
    }
    SessionEnvironment::new(host.cwd().to_path_buf(), merged)
}

/// Reports whether the persisted environment should be restored for this session.
fn inheritance_enabled(variables: &[(OsString, OsString)]) -> bool {
    variables
        .iter()
        .find(|(name, _)| crate::session::environment_name_eq(name, INHERIT_ENV))
        .and_then(|(_, value)| value.to_str())
        .is_none_or(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        })
}

/// Lays the host environment over the persisted one, keeping the host authoritative.
fn overlay(
    persisted: Vec<(OsString, OsString)>,
    host: &[(OsString, OsString)],
) -> Vec<(OsString, OsString)> {
    let mut merged: Vec<(OsString, OsString)> = Vec::with_capacity(persisted.len() + host.len());
    for (name, value) in persisted {
        if merged
            .iter()
            .any(|(existing, _)| crate::session::environment_name_eq_os(existing, &name))
        {
            continue;
        }
        merged.push((name, value));
    }
    for (name, value) in host {
        match merged
            .iter_mut()
            .find(|(existing, _)| crate::session::environment_name_eq_os(existing, name))
        {
            Some(entry) => {
                entry.1 = if crate::session::environment_name_eq(name, SEARCH_PATH) {
                    union_search_path(value, &entry.1)
                } else {
                    value.clone()
                };
            }
            None => merged.push((name.clone(), value.clone())),
        }
    }
    merged
}

/// Appends persisted search-path entries the host's own search path does not already contain.
///
/// The host value is never rewritten: it is the live path, it comes first, and anything it already
/// resolves keeps resolving to the same place. Only genuinely absent directories are added.
fn union_search_path(host: &OsStr, persisted: &OsStr) -> OsString {
    let host_entries = split_search_path(host)
        .map(comparable_entry)
        .collect::<Vec<_>>();
    let mut merged = host.to_os_string();
    for entry in split_search_path(persisted) {
        let comparable = comparable_entry(entry);
        if comparable.is_empty() || host_entries.contains(&comparable) {
            continue;
        }
        if !merged.is_empty() {
            merged.push(PATH_LIST_SEPARATOR.to_string());
        }
        merged.push(entry);
    }
    merged
}

fn split_search_path(value: &OsStr) -> impl Iterator<Item = &OsStr> {
    value
        .to_str()
        .into_iter()
        .flat_map(|value| value.split(PATH_LIST_SEPARATOR))
        .map(OsStr::new)
}

/// Normalizes one search-path entry so that spelling differences do not duplicate a directory.
fn comparable_entry(entry: &OsStr) -> String {
    let entry = entry.to_string_lossy();
    let trimmed = entry.trim().trim_end_matches(['/', '\\']);
    if cfg!(windows) {
        trimmed.to_ascii_lowercase().replace('\\', "/")
    } else {
        trimmed.to_string()
    }
}

/// Expands `%NAME%` references against an already-resolved lookup, leaving unknown names literal.
///
/// Values that are not valid Unicode are returned unchanged rather than mangled; Windows stores
/// expandable strings as UTF-16, so a value that fails this test cannot contain a usable reference.
#[cfg(windows)]
fn expand_references(value: &OsStr, lookup: &[(OsString, OsString)]) -> OsString {
    let Some(value) = value.to_str() else {
        return value.to_os_string();
    };
    if !value.contains('%') {
        return OsString::from(value);
    }
    let mut expanded = OsString::new();
    let mut rest = value;
    while let Some(start) = rest.find('%') {
        expanded.push(&rest[..start]);
        let tail = &rest[start + 1..];
        let Some(end) = tail.find('%') else {
            expanded.push(&rest[start..]);
            return expanded;
        };
        let name = &tail[..end];
        match lookup
            .iter()
            .find(|(candidate, _)| crate::session::environment_name_eq(candidate, name))
        {
            Some((_, resolved)) => expanded.push(resolved),
            None => {
                expanded.push("%");
                expanded.push(name);
                expanded.push("%");
            }
        }
        rest = &tail[end + 1..];
    }
    expanded.push(rest);
    expanded
}

#[cfg(not(windows))]
/// Unix persists a user's environment in the login profile, which `bash -lc` already sources.
fn persisted_environment(_host: &[(OsString, OsString)]) -> Vec<(OsString, OsString)> {
    Vec::new()
}

#[cfg(windows)]
fn persisted_environment(host: &[(OsString, OsString)]) -> Vec<(OsString, OsString)> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    // System values are laid down first so the user's own hive overrides them, matching how
    // Windows composes a logon environment. PATH is unioned rather than replaced for both.
    let system = read_environment_key(
        HKEY_LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
    );
    let user = read_environment_key(HKEY_CURRENT_USER, "Environment");
    let merged = overlay(system, &user);
    // References are resolved against the host environment as well: names computed at logon —
    // `USERPROFILE`, `SystemRoot` — are never stored in these keys, so a lookup limited to the
    // registry would leave `%USERPROFILE%\go` as literal text and hand a command a broken path.
    // Every value is resolved rather than only the ones stored as expandable: a plain string
    // holding a resolvable `%NAME%` is a mistyped entry, and expanding it is what its author meant.
    let lookup = overlay(merged.clone(), host);
    merged
        .into_iter()
        .map(|(name, value)| {
            let value = expand_references(&value, &lookup);
            (name, value)
        })
        .collect()
}

#[cfg(windows)]
fn read_environment_key(
    root: windows_sys::Win32::System::Registry::HKEY,
    subkey: &str,
) -> Vec<(OsString, OsString)> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, KEY_READ, REG_EXPAND_SZ, REG_SZ, RegCloseKey, RegEnumValueW, RegOpenKeyExW,
    };

    // Registry value names are bounded at 16383 characters, so one maximum-size buffer removes
    // the retry path that RegEnumValueW does not report a required name length for.
    const MAX_VALUE_NAME: usize = 16_384;

    let wide_subkey = OsStr::new(subkey)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<u16>>();
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: the subkey is NUL-terminated and `key` receives a handle closed below.
    let status =
        unsafe { RegOpenKeyExW(root, wide_subkey.as_ptr(), 0, KEY_READ, &raw mut key) } as u32;
    if status != ERROR_SUCCESS {
        // An unreadable key degrades to the environment the host supplied, which is what every
        // release before this one used. Failing the call instead would take the shell tools down
        // over an enhancement, so absence is reported as "nothing persisted" rather than an error.
        return Vec::new();
    }

    let mut values = Vec::new();
    let mut name = vec![0u16; MAX_VALUE_NAME];
    let mut data = vec![0u8; 4096];
    let mut index = 0u32;
    loop {
        let mut name_length = name.len() as u32;
        let mut kind = 0u32;
        let mut data_length = data.len() as u32;
        // SAFETY: every pointer refers to a live buffer whose length is passed alongside it.
        let status = unsafe {
            RegEnumValueW(
                key,
                index,
                name.as_mut_ptr(),
                &raw mut name_length,
                std::ptr::null_mut(),
                &raw mut kind,
                data.as_mut_ptr(),
                &raw mut data_length,
            )
        } as u32;
        if status == ERROR_MORE_DATA {
            // Only the data buffer can be short; it now holds the required byte count.
            data.resize((data_length as usize).max(data.len() * 2), 0);
            continue;
        }
        if status != ERROR_SUCCESS {
            break;
        }
        index += 1;
        if kind != REG_SZ && kind != REG_EXPAND_SZ {
            continue;
        }
        let name_value = OsString::from_wide(&name[..name_length as usize]);
        if name_value.is_empty() {
            continue;
        }
        values.push((name_value, wide_value(&data[..data_length as usize])));
    }
    // SAFETY: `key` was opened above and is not used afterwards.
    unsafe { RegCloseKey(key) };
    values
}

/// Decodes a registry string payload, dropping the terminator the API counts but callers must not keep.
#[cfg(windows)]
fn wide_value(bytes: &[u8]) -> OsString {
    use std::os::windows::ffi::OsStringExt;
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect::<Vec<u16>>();
    let end = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    OsString::from_wide(&units[..end])
}

#[cfg(test)]
mod tests {
    use super::{command_environment, inheritance_enabled, overlay, union_search_path};
    use crate::session::SessionEnvironment;
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn pairs(entries: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        entries
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    fn value<'a>(merged: &'a [(OsString, OsString)], name: &str) -> Option<&'a OsString> {
        merged
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value)
    }

    #[test]
    fn the_host_wins_every_name_it_provides_and_absent_names_survive() {
        let merged = overlay(
            pairs(&[("JAVA_HOME", "C:/jdk21"), ("TEMP", "C:/persisted-temp")]),
            &pairs(&[("TEMP", "C:/live-temp"), ("FASTCTX_TOKEN_BUDGET", "9000")]),
        );
        assert_eq!(value(&merged, "JAVA_HOME").unwrap(), "C:/jdk21");
        assert_eq!(value(&merged, "TEMP").unwrap(), "C:/live-temp");
        assert_eq!(value(&merged, "FASTCTX_TOKEN_BUDGET").unwrap(), "9000");
        assert_eq!(
            merged.len(),
            3,
            "a name must never appear twice: {merged:?}"
        );
    }

    #[test]
    fn the_search_path_keeps_the_live_value_first_and_only_gains_absent_entries() {
        // Entries carry no drive letter: off Windows a colon is the list separator, so a
        // `C:/...` fixture would be read as two entries there and prove nothing.
        let separator = super::PATH_LIST_SEPARATOR;
        let host = format!("/live/bin{separator}/Shared/Bin");
        let persisted = format!("/shared/bin/{separator}/persisted/only{separator}");
        let merged = union_search_path(&OsString::from(&host), &OsString::from(&persisted));
        let merged = merged.to_string_lossy();
        assert!(
            merged.starts_with(&host),
            "live path was rewritten: {merged}"
        );
        assert!(merged.ends_with("/persisted/only"), "{merged}");
        // Windows compares entries case-insensitively and ignores a trailing separator, so the
        // shared directory must not reappear under a second spelling; elsewhere case matters.
        let occurrences = merged.to_ascii_lowercase().matches("shared/bin").count();
        let expected = if cfg!(windows) { 1 } else { 2 };
        assert_eq!(occurrences, expected, "{merged}");
    }

    #[cfg(windows)]
    #[test]
    fn unresolvable_references_stay_literal_instead_of_expanding_to_nothing() {
        use super::expand_references;
        let lookup = pairs(&[("SystemRoot", "C:/Windows")]);
        assert_eq!(
            expand_references(&OsString::from("%SystemRoot%/System32"), &lookup),
            OsString::from("C:/Windows/System32")
        );
        assert_eq!(
            expand_references(&OsString::from("%NOT_SET%/bin"), &lookup),
            OsString::from("%NOT_SET%/bin")
        );
        assert_eq!(
            expand_references(&OsString::from("100% done"), &lookup),
            OsString::from("100% done")
        );
    }

    #[test]
    fn the_opt_out_is_honoured_only_for_values_that_clearly_mean_off() {
        for setting in ["0", "false", "OFF", "no", " No "] {
            assert!(
                !inheritance_enabled(&pairs(&[(super::INHERIT_ENV, setting)])),
                "{setting}"
            );
        }
        for setting in ["1", "true", "on", "yes", ""] {
            assert!(
                inheritance_enabled(&pairs(&[(super::INHERIT_ENV, setting)])),
                "{setting}"
            );
        }
        assert!(inheritance_enabled(&pairs(&[("PATH", "C:/live/bin")])));
    }

    /// Holds on whichever machine runs it: nothing is persisted off Windows, and on Windows the
    /// real registry is read — either way the session's own cwd and search path lead the result.
    #[test]
    fn a_derived_environment_keeps_the_session_cwd_and_leads_with_the_live_search_path() {
        let host = SessionEnvironment::new(
            PathBuf::from("."),
            pairs(&[("PATH", "/live/bin"), ("FASTCTX_TOKEN_BUDGET", "9000")]),
        );
        let derived = command_environment(&host);
        assert_eq!(derived.cwd(), host.cwd());
        assert!(
            derived
                .var_os("PATH")
                .is_some_and(|path| path.to_string_lossy().starts_with("/live/bin")),
            "the live search path must survive verbatim as the prefix"
        );
        assert_eq!(
            derived.var("FASTCTX_TOKEN_BUDGET").unwrap(),
            "9000",
            "a host-provided value must never be displaced by a persisted one"
        );
    }
}
