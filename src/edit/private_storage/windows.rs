//! Windows private-directory ACLs and reparse-point enforcement.

use std::fs::{self, OpenOptions};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Path, PathBuf};

pub(super) fn runtime_component(component: &str) -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|path| path.join("AppData").join("Local"))
        })
        .unwrap_or_else(std::env::temp_dir);
    base.join("fastctx").join("runtime").join(component)
}

pub(super) fn ensure_private_directory(path: &Path, label: &str) -> Result<(), String> {
    let user_sid = current_process_user_sid_string()?;
    let descriptor = PrivateAcl::new(path, label, &user_sid)?;
    let chain = managed_chain(path);
    let mut opened_chain = Vec::with_capacity(chain.len());
    for directory in &chain {
        let opened = match open_directory_for_security(directory, label) {
            Ok(opened) => opened,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                descriptor.create_directory(directory, label)?;
                open_directory_for_security(directory, label).map_err(|error| {
                    directory_io_error("open the newly created", directory, label, error)
                })?
            }
            Err(error) => return Err(directory_io_error("open", directory, label, error)),
        };
        descriptor.secure_open_directory(&opened, directory, label)?;
        opened_chain.push(opened);
    }
    super::verify_directory(path, label)
}

fn managed_chain(path: &Path) -> Vec<PathBuf> {
    let root = path
        .ancestors()
        .find_map(|ancestor| {
            let parent = ancestor.parent()?;
            if file_name_eq(ancestor, "runtime") && file_name_eq(parent, "fastctx") {
                Some(parent)
            } else if file_name_eq(ancestor, ".fastctx") {
                Some(ancestor)
            } else {
                None
            }
        })
        .unwrap_or(path);
    let mut chain = Vec::new();
    for ancestor in path.ancestors() {
        chain.push(ancestor.to_path_buf());
        if ancestor == root {
            break;
        }
    }
    chain.reverse();
    chain
}

fn file_name_eq(path: &Path, expected: &str) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(expected))
}

fn current_process_user_sid_string() -> Result<String, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token: HANDLE = std::ptr::null_mut();
    let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if opened == 0 {
        return Err(format!(
            "Cannot inspect the current Windows user for private runtime storage: {}",
            std::io::Error::last_os_error()
        ));
    }

    let result = (|| {
        let mut required = 0_u32;
        unsafe {
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut required);
        }
        if required < std::mem::size_of::<TOKEN_USER>() as u32 {
            return Err(format!(
                "Cannot size the current Windows user identity: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut buffer = aligned_buffer(required);
        let read = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                required,
                &mut required,
            )
        };
        if read == 0 {
            return Err(format!(
                "Cannot read the current Windows user identity: {}",
                std::io::Error::last_os_error()
            ));
        }
        let token_user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        let mut string_sid = std::ptr::null_mut();
        let converted = unsafe { ConvertSidToStringSidW(token_user.User.Sid, &mut string_sid) };
        if converted == 0 {
            return Err(format!(
                "Cannot format the current Windows user identity: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut length = 0_usize;
        unsafe {
            while *string_sid.add(length) != 0 {
                length += 1;
            }
        }
        let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(string_sid, length) })
            .map_err(|_| "Windows returned a malformed user SID string.".to_string());
        unsafe {
            windows_sys::Win32::Foundation::LocalFree(string_sid.cast());
        }
        sid
    })();
    unsafe {
        CloseHandle(token);
    }
    result
}

fn aligned_buffer(required_bytes: u32) -> Vec<usize> {
    let word_size = std::mem::size_of::<usize>();
    vec![0_usize; (required_bytes as usize).div_ceil(word_size)]
}

struct PrivateAcl {
    descriptor: windows_sys::Win32::Security::PSECURITY_DESCRIPTOR,
    dacl: *mut windows_sys::Win32::Security::ACL,
}

impl PrivateAcl {
    fn new(path: &Path, label: &str, user_sid: &str) -> Result<Self, String> {
        use windows_sys::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
        };
        use windows_sys::Win32::Security::GetSecurityDescriptorDacl;

        // 2026-07-22: TokenUser is stable across UAC's elevated and filtered tokens;
        // Owner Rights can resolve to Administrators and lock the filtered token out.
        let system_ace = if user_sid.eq_ignore_ascii_case("S-1-5-18") {
            ""
        } else {
            "(A;OICI;FA;;;SY)"
        };
        let descriptor_text = format!("D:P(A;OICI;FA;;;{user_sid}){system_ace}")
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let mut descriptor = std::ptr::null_mut();
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                descriptor_text.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if converted == 0 {
            return Err(format!(
                "Cannot build the private ACL for the {label} directory {}: {}",
                crate::paths::display_path(path),
                std::io::Error::last_os_error()
            ));
        }

        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl = std::ptr::null_mut();
        let read = unsafe {
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted)
        };
        if read == 0 || present == 0 || dacl.is_null() {
            let error = std::io::Error::last_os_error();
            unsafe {
                windows_sys::Win32::Foundation::LocalFree(descriptor);
            }
            return Err(format!(
                "Cannot inspect the private ACL for the {label} directory {}: {error}",
                crate::paths::display_path(path)
            ));
        }
        Ok(Self { descriptor, dacl })
    }

    fn create_directory(&self, path: &Path, label: &str) -> Result<(), String> {
        use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
        use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;

        let wide = wide_path(path);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor,
            bInheritHandle: 0,
        };
        let created = unsafe { CreateDirectoryW(wide.as_ptr(), &attributes) };
        if created != 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            return Ok(());
        }
        Err(directory_io_error("create", path, label, error))
    }

    fn secure_open_directory(
        &self,
        directory: &fs::File,
        path: &Path,
        label: &str,
    ) -> Result<(), String> {
        use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetSecurityInfo};
        use windows_sys::Win32::Security::{
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        };

        let result = unsafe {
            SetSecurityInfo(
                directory.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                self.dacl,
                std::ptr::null(),
            )
        };
        if result != 0 {
            return Err(directory_io_error(
                "secure",
                path,
                label,
                std::io::Error::from_raw_os_error(result as i32),
            ));
        }
        Ok(())
    }
}

impl Drop for PrivateAcl {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::LocalFree(self.descriptor);
        }
    }
}

fn open_directory_for_security(path: &Path, label: &str) -> Result<fs::File, std::io::Error> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, READ_CONTROL,
        WRITE_DAC,
    };

    let wide = wide_path(path);
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    let directory = unsafe { fs::File::from_raw_handle(handle) };
    let metadata = directory.metadata()?;
    if let Err(message) = super::validate_directory_metadata(path, label, &metadata) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            message,
        ));
    }
    Ok(directory)
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn directory_io_error(action: &str, path: &Path, label: &str, error: std::io::Error) -> String {
    let mut message = format!(
        "Cannot {action} the {label} directory {}: {error}",
        crate::paths::display_path(path)
    );
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        message.push_str(
            "\nNote: an earlier FastCtx version may have left an owner-only Windows ACL. Retry this operation once from an elevated process under the same Windows account; FastCtx can repair that ACL in place and never deletes the directory. If access is still denied, restore that account's Full control on the listed FastCtx directory or ask an administrator to do so.",
        );
    }
    message
}

pub(super) fn configure_no_follow(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

pub(super) fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(test)]
mod tests {
    use super::{
        aligned_buffer, directory_io_error, ensure_private_directory, managed_chain, wide_path,
    };
    use std::mem::size_of;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        CONTAINER_INHERIT_ACE, CreateWellKnownSid, DACL_SECURITY_INFORMATION, EqualSid, GetAce,
        GetAclInformation, GetFileSecurityW, GetSecurityDescriptorControl,
        GetSecurityDescriptorDacl, GetTokenInformation, OBJECT_INHERIT_ACE,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
        SetFileSecurityW, TOKEN_QUERY, TOKEN_USER, TokenUser, WinLocalSystemSid,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    const ACCESS_ALLOWED_ACE_TYPE_VALUE: u8 = 0;

    struct SidBuffer {
        _storage: Vec<usize>,
        sid: PSID,
    }

    fn current_user_sid() -> SidBuffer {
        let mut token: HANDLE = std::ptr::null_mut();
        assert_ne!(
            unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) },
            0,
            "{}",
            std::io::Error::last_os_error()
        );
        let mut required = 0_u32;
        unsafe {
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut required);
        }
        assert!(required >= size_of::<TOKEN_USER>() as u32);
        let mut storage = aligned_buffer(required);
        assert_ne!(
            unsafe {
                GetTokenInformation(
                    token,
                    TokenUser,
                    storage.as_mut_ptr().cast(),
                    required,
                    &mut required,
                )
            },
            0,
            "{}",
            std::io::Error::last_os_error()
        );
        unsafe {
            CloseHandle(token);
        }
        let token_user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
        SidBuffer {
            sid: token_user.User.Sid,
            _storage: storage,
        }
    }

    fn local_system_sid() -> SidBuffer {
        let mut required = 0_u32;
        unsafe {
            CreateWellKnownSid(
                WinLocalSystemSid,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut required,
            );
        }
        assert!(required > 0);
        let mut storage = aligned_buffer(required);
        let sid = storage.as_mut_ptr().cast();
        assert_ne!(
            unsafe {
                CreateWellKnownSid(WinLocalSystemSid, std::ptr::null_mut(), sid, &mut required)
            },
            0,
            "{}",
            std::io::Error::last_os_error()
        );
        SidBuffer {
            _storage: storage,
            sid,
        }
    }

    fn security_descriptor(path: &Path) -> Vec<usize> {
        let wide = wide_path(path);
        let mut required = 0_u32;
        unsafe {
            GetFileSecurityW(
                wide.as_ptr(),
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                0,
                &mut required,
            );
        }
        assert!(required > 0, "{}", std::io::Error::last_os_error());
        let mut descriptor = aligned_buffer(required);
        assert_ne!(
            unsafe {
                GetFileSecurityW(
                    wide.as_ptr(),
                    DACL_SECURITY_INFORMATION,
                    descriptor.as_mut_ptr().cast(),
                    required,
                    &mut required,
                )
            },
            0,
            "{}",
            std::io::Error::last_os_error()
        );
        descriptor
    }

    fn dacl_from_descriptor(descriptor: &[usize]) -> *mut ACL {
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl = std::ptr::null_mut();
        assert_ne!(
            unsafe {
                GetSecurityDescriptorDacl(
                    descriptor.as_ptr().cast_mut().cast(),
                    &mut present,
                    &mut dacl,
                    &mut defaulted,
                )
            },
            0,
            "{}",
            std::io::Error::last_os_error()
        );
        assert_ne!(present, 0);
        assert!(!dacl.is_null());
        dacl
    }

    fn is_dacl_protected(path: &Path) -> bool {
        let descriptor = security_descriptor(path);
        let mut control = 0_u16;
        let mut revision = 0_u32;
        assert_ne!(
            unsafe {
                GetSecurityDescriptorControl(
                    descriptor.as_ptr().cast_mut().cast(),
                    &mut control,
                    &mut revision,
                )
            },
            0,
            "{}",
            std::io::Error::last_os_error()
        );
        control & SE_DACL_PROTECTED != 0
    }

    fn assert_private_acl(path: &Path) {
        let descriptor = security_descriptor(path);
        let mut control = 0_u16;
        let mut revision = 0_u32;
        assert_ne!(
            unsafe {
                GetSecurityDescriptorControl(
                    descriptor.as_ptr().cast_mut().cast(),
                    &mut control,
                    &mut revision,
                )
            },
            0
        );
        assert_ne!(control & SE_DACL_PROTECTED, 0);

        let dacl = dacl_from_descriptor(&descriptor);
        let mut information = ACL_SIZE_INFORMATION::default();
        assert_ne!(
            unsafe {
                GetAclInformation(
                    dacl,
                    (&mut information as *mut ACL_SIZE_INFORMATION).cast(),
                    size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                )
            },
            0
        );
        let user = current_user_sid();
        let system = local_system_sid();
        let user_is_system = unsafe { EqualSid(user.sid, system.sid) } != 0;
        assert_eq!(information.AceCount, if user_is_system { 1 } else { 2 });
        let mut saw_user = false;
        let mut saw_system = false;
        for index in 0..information.AceCount {
            let mut ace = std::ptr::null_mut();
            assert_ne!(unsafe { GetAce(dacl, index, &mut ace) }, 0);
            let header = unsafe { std::ptr::read_unaligned(ace.cast::<ACE_HEADER>()) };
            assert_eq!(header.AceType, ACCESS_ALLOWED_ACE_TYPE_VALUE);
            assert_eq!(
                header.AceFlags,
                (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE) as u8
            );
            assert!(header.AceSize as usize >= size_of::<ACCESS_ALLOWED_ACE>());
            let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
            let mask = unsafe { std::ptr::addr_of!((*allowed).Mask).read_unaligned() };
            assert_eq!(mask, FILE_ALL_ACCESS);
            let sid = unsafe { std::ptr::addr_of!((*allowed).SidStart).cast_mut().cast() };
            let matches_user = unsafe { EqualSid(sid, user.sid) } != 0;
            let matches_system = unsafe { EqualSid(sid, system.sid) } != 0;
            if user_is_system {
                assert!(matches_user && matches_system, "unexpected ACE SID");
            } else {
                assert_ne!(matches_user, matches_system, "unexpected ACE SID");
            }
            assert!(!(matches_user && saw_user), "duplicate TokenUser ACE");
            assert!(!(matches_system && saw_system), "duplicate LocalSystem ACE");
            saw_user |= matches_user;
            saw_system |= matches_system;
        }
        assert!(saw_user && saw_system);
    }

    fn apply_legacy_owner_rights_acl(path: &Path) {
        let descriptor_text = "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)"
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    descriptor_text.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let wide = wide_path(path);
        assert_ne!(
            unsafe {
                SetFileSecurityW(
                    wide.as_ptr(),
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    descriptor,
                )
            },
            0,
            "{}",
            std::io::Error::last_os_error()
        );
        unsafe {
            LocalFree(descriptor);
        }
    }

    fn create_junction(target: &Path, link: &Path) {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

        let command = std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into());
        let output = std::process::Command::new(command)
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "cannot create test junction: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn private_acl_is_protected_and_contains_only_token_user_and_system() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("fastctx/runtime/edit-locks");
        ensure_private_directory(&directory, "edit-lock").unwrap();
        for managed in managed_chain(&directory) {
            assert_private_acl(&managed);
        }
    }

    #[test]
    fn generic_private_storage_creates_missing_parent_chain_privately() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join(".fastctx");
        let directory = parent.join("jobs");
        ensure_private_directory(&directory, "background job registry").unwrap();
        assert_private_acl(&parent);
        assert_private_acl(&directory);
    }

    #[test]
    fn managed_chain_rejects_a_reparse_anchor_before_creating_children() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let outside = temp.path().join("outside");
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let anchor = home.join(".fastctx");
        create_junction(&outside, &anchor);

        let directory = anchor.join("jobs");
        let error = ensure_private_directory(&directory, "background job registry").unwrap_err();
        assert!(error.contains("not a private directory"), "{error}");
        assert!(!outside.join("jobs").exists());
        std::fs::remove_dir(anchor).unwrap();
    }

    #[test]
    fn legacy_owner_rights_acl_is_replaced_in_place() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("fastctx/runtime/edit-locks");
        std::fs::create_dir_all(&directory).unwrap();
        let sentinel = directory.join("existing.lock");
        std::fs::write(&sentinel, b"keep").unwrap();
        for managed in managed_chain(&directory).into_iter().rev() {
            apply_legacy_owner_rights_acl(&managed);
        }

        ensure_private_directory(&directory, "edit-lock").unwrap();

        assert_eq!(std::fs::read(sentinel).unwrap(), b"keep");
        for managed in managed_chain(&directory) {
            assert_private_acl(&managed);
        }
    }

    #[test]
    fn managed_chain_ignores_an_earlier_profile_component_named_fastctx() {
        let path = PathBuf::from(r"C:\fastctx\profile\AppData\Local\fastctx\runtime\edit-locks");
        assert_eq!(
            managed_chain(&path),
            vec![
                PathBuf::from(r"C:\fastctx\profile\AppData\Local\fastctx"),
                PathBuf::from(r"C:\fastctx\profile\AppData\Local\fastctx\runtime"),
                path,
            ]
        );
    }

    #[test]
    fn permission_denied_error_has_an_in_place_recovery_path() {
        let message = directory_io_error(
            "open",
            Path::new(r"C:\Users\tester\AppData\Local\fastctx"),
            "edit-lock",
            std::io::Error::from_raw_os_error(5),
        );
        assert!(message.contains("owner-only Windows ACL"), "{message}");
        assert!(message.contains("elevated process under the same Windows account"));
        assert!(message.contains("repair that ACL in place"));
        assert!(message.contains("never deletes the directory"));
        assert!(message.contains("If access is still denied"));
    }

    #[test]
    #[ignore]
    fn issue10_acl_transition_probe() {
        let Some(path) = std::env::var_os("FASTCTX_ISSUE10_PROBE_PATH") else {
            return;
        };
        let output = std::env::var_os("FASTCTX_ISSUE10_PROBE_OUTPUT").unwrap();
        let path = Path::new(&path);
        let result: Result<(), String> = (|| {
            for iteration in 0..64 {
                ensure_private_directory(path, "edit-lock")
                    .map_err(|error| format!("iteration {iteration}: {error}"))?;
                let lock_path = path.join("transition-probe.lock");
                super::super::open_lock_file(&lock_path, "edit lock")
                    .map_err(|error| format!("iteration {iteration}: {error}"))?;
            }
            Ok(())
        })();
        let message = match &result {
            Ok(()) => "ok".to_string(),
            Err(error) => format!("error: {error}"),
        };
        std::fs::write(output, message).unwrap();
        result.unwrap();
    }

    #[test]
    fn directory_reparse_point_is_rejected_without_touching_its_target() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("outside");
        std::fs::create_dir(&target).unwrap();
        let sentinel = target.join("sentinel");
        std::fs::write(&sentinel, b"outside").unwrap();
        let protected_before = is_dacl_protected(&target);
        let link = temp.path().join("edit-locks");
        create_junction(&target, &link);

        let error = ensure_private_directory(&link, "edit-lock").unwrap_err();
        assert!(error.contains("not a private directory"), "{error}");
        assert_eq!(std::fs::read(sentinel).unwrap(), b"outside");
        assert_eq!(is_dacl_protected(&target), protected_before);
        std::fs::remove_dir(link).unwrap();
    }
}
