//! A terminal opened after installation finds the placed fastctx. Windows
//! appends its directory to the user's registry PATH. Linux links it into
//! ~/.local/bin, which Ubuntu's login profile adds without editing the profile.
//! macOS links it there too, and its login shell, zsh, is told of that folder
//! by two marked lines in ~/.zprofile, which uninstalling takes out again.

use std::path::Path;

/// What putting a directory on the PATH came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Added {
    /// An entry already names it.
    Already,
    Added,
}

/// Puts `dir` on this user's PATH.
#[cfg(windows)]
pub fn add(dir: &Path) -> Result<Added, String> {
    let key =
        windows_registry::CURRENT_USER.create("Environment").map_err(|error| format!("HKCU\\Environment: {error}"))?;
    let added = add_to(&key, dir)?;
    if added == Added::Added {
        announce();
    }
    Ok(added)
}

#[cfg(unix)]
pub fn add(dir: &Path) -> Result<Added, String> {
    use std::os::unix::fs::symlink;
    let home = fastctx_platform::user_home().map_err(|error| error.to_string())?;
    let bin = home.join(".local/bin");
    std::fs::create_dir_all(&bin).map_err(|error| error.to_string())?;
    let link = bin.join("fastctx");
    let target = dir.join("fastctx");
    #[cfg(target_os = "macos")]
    on_login_path(&home)?;
    match symlink(&target, &link) {
        Ok(()) => Ok(Added::Added),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if std::fs::read_link(&link).ok().as_ref() == Some(&target) {
                Ok(Added::Already)
            } else {
                Err(format!("{} already names another program", link.display()))
            }
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Puts ~/.local/bin on zsh's login PATH, unless ~/.zprofile names it.
#[cfg(target_os = "macos")]
fn on_login_path(home: &Path) -> Result<(), String> {
    let profile = home.join(".zprofile");
    let text = match std::fs::read_to_string(&profile) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("reading {}: {error}", profile.display())),
    };
    if text.contains(".local/bin") {
        return Ok(());
    }
    let separator = if text.is_empty() || text.ends_with('\n') { "" } else { "\n" };
    let lines = fastctx_install::uninstall::ZPROFILE_LINES;
    std::fs::write(&profile, format!("{text}{separator}{lines}"))
        .map_err(|error| format!("writing {}: {error}", profile.display()))
}

/// Appends `dir` to the `Path` value under `key` unless an entry already
/// names it. The value keeps its type (a new one is `REG_EXPAND_SZ`, as
/// Windows makes it) and every character it had.
#[cfg(windows)]
pub fn add_to(key: &windows_registry::Key, dir: &Path) -> Result<Added, String> {
    use windows_registry::Type;
    let dir = dir.to_str().ok_or_else(|| format!("{} is not Unicode", dir.display()))?;
    let (kind, old) = match key.get_type("Path") {
        Ok(kind @ (Type::String | Type::ExpandString)) => {
            (kind, key.get_string("Path").map_err(|error| format!("reading Path: {error}"))?)
        }
        Ok(other) => return Err(format!("Path holds a {other:?}, not a string; it is left as it is")),
        Err(error) if error.code() == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND.to_hresult() => {
            (Type::ExpandString, String::new())
        }
        Err(error) => return Err(format!("reading Path: {error}")),
    };
    let wanted = normal(dir);
    if old.split(';').any(|entry| !entry.trim().is_empty() && normal(&expand(entry.trim())) == wanted) {
        return Ok(Added::Already);
    }
    let new = if old.is_empty() || old.ends_with(';') { format!("{old}{dir}") } else { format!("{old};{dir}") };
    match kind {
        Type::String => key.set_string("Path", &new),
        _ => key.set_expand_string("Path", &new),
    }
    .map_err(|error| format!("writing Path: {error}"))?;
    Ok(Added::Added)
}

/// A directory as Windows compares them: case and the kind of slash do not
/// matter, nor a trailing one.
#[cfg(windows)]
fn normal(path: &str) -> String {
    path.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// `%NAME%` written into an entry, as Windows expands it.
#[cfg(windows)]
fn expand(entry: &str) -> String {
    use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
    use windows::core::HSTRING;
    let source = HSTRING::from(entry);
    // SAFETY: a null buffer asks for the size the expansion needs.
    let needed = unsafe { ExpandEnvironmentStringsW(&source, None) } as usize;
    if needed == 0 {
        return entry.to_string();
    }
    let mut buffer = vec![0u16; needed];
    // SAFETY: the buffer holds the size asked for above.
    let written = unsafe { ExpandEnvironmentStringsW(&source, Some(&mut buffer)) } as usize;
    if written == 0 || written > buffer.len() {
        return entry.to_string();
    }
    String::from_utf16_lossy(&buffer[..written - 1])
}

/// Tells the windows that read the environment, Explorer first, that it
/// changed, so programs started from them afterwards see the new PATH.
#[cfg(windows)]
fn announce() {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };
    use windows::core::w;
    // A window that does not answer is passed over after five seconds, as
    // Microsoft's documentation of environment variables broadcasts it.
    // SAFETY: the message's text is a static wide string that outlives the call.
    unsafe {
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(w!("Environment").as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            5000,
            None,
        );
    }
}
