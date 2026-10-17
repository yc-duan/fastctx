//! The home Fastctx gives SYSTEM, which runs a managed Windows computer's
//! node when a remote management tool installed it (`world-network.md`,
//! 登录与入网). SYSTEM's own profile is inside System32, where a 32-bit
//! program is shown another folder of the same name, so the node, its
//! agents and every command it runs live in `%ProgramData%\Fastctx`
//! instead, which only SYSTEM and the Administrators group may enter.
//! Anywhere else, and for any other account, there is no such home.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

/// SYSTEM's home, when this process runs as SYSTEM on Windows.
pub fn home() -> io::Result<Option<PathBuf>> {
    imp::home()
}

/// The variables that point a process at SYSTEM's home: its profile,
/// `HOME` as Git's shell reads it, the two application data folders and
/// the temporary one. Empty for any other account.
pub fn variables() -> io::Result<Vec<(OsString, OsString)>> {
    let Some(home) = home()? else { return Ok(Vec::new()) };
    let roaming = home.join("AppData").join("Roaming");
    let local = home.join("AppData").join("Local");
    let temporary = local.join("Temp");
    Ok([
        ("USERPROFILE", home.clone()),
        ("HOME", home),
        ("APPDATA", roaming),
        ("LOCALAPPDATA", local),
        ("TEMP", temporary.clone()),
        ("TMP", temporary),
    ]
    .into_iter()
    .map(|(name, value)| (name.into(), value.into_os_string()))
    .collect())
}

/// Makes SYSTEM's home and the folders its variables name, the home with
/// its protected access list from the first instant, and points this
/// process at it. Nothing for any other account.
pub fn prepare() -> io::Result<()> {
    imp::prepare()?;
    adopt()
}

/// Points this process's own variables at SYSTEM's home, so what it starts
/// without the World's environment finds the same one. Only before other
/// threads read the environment: at the start of a command.
pub fn adopt() -> io::Result<()> {
    for (name, value) in variables()? {
        // SAFETY: called at the start of a command, before it starts threads.
        unsafe { std::env::set_var(name, value) };
    }
    Ok(())
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    const SYSTEM: &str = "S-1-5-18";
    /// Owned by SYSTEM; SYSTEM and the Administrators group have full
    /// control, inherited by everything inside; nobody else has any.
    const ACCESS: &str = "O:SYG:SYD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";

    pub fn home() -> io::Result<Option<PathBuf>> {
        static SYSTEM_HOME: OnceLock<Option<PathBuf>> = OnceLock::new();
        if let Some(home) = SYSTEM_HOME.get() {
            return Ok(home.clone());
        }
        let home = if crate::current_user_sid()? == SYSTEM {
            Some(crate::protected_directory::program_data()?.join("Fastctx"))
        } else {
            None
        };
        Ok(SYSTEM_HOME.get_or_init(|| home).clone())
    }

    pub fn prepare() -> io::Result<()> {
        let Some(home) = home()? else { return Ok(()) };
        crate::protected_directory::ensure(&home, ACCESS)?;
        for (_, folder) in super::variables()? {
            std::fs::create_dir_all(folder)?;
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    use std::io;
    use std::path::PathBuf;

    pub fn home() -> io::Result<Option<PathBuf>> {
        Ok(None)
    }

    pub fn prepare() -> io::Result<()> {
        Ok(())
    }
}
