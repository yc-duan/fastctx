//! The installer owns the unpackaged app identity. Running the GUI never
//! rewrites a protocol registration or a Start menu shortcut.

use std::path::PathBuf;

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::StructuredStorage::{
    InitPropVariantFromCLSID, PROPVARIANT, PVCHF_DEFAULT, PropVariantChangeType,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink};
use windows::Win32::UI::Shell::{
    SHCNE_ASSOCCHANGED, SHCNE_UPDATEDIR, SHCNF_FLUSH, SHCNF_IDLIST, SHCNF_PATHW, SHChangeNotify,
};
use windows::core::{BSTR, GUID, HSTRING, Interface};
use windows_registry::CURRENT_USER;

#[cfg(not(debug_assertions))]
pub const APP_ID: &str = "dev.fastctx.world";
#[cfg(debug_assertions)]
pub const APP_ID: &str = "dev.fastctx.world.development";
#[cfg(not(debug_assertions))]
pub const SCHEME: &str = "fastctx-world";
#[cfg(debug_assertions)]
pub const SCHEME: &str = "fastctx-world-development";
#[cfg(not(debug_assertions))]
const SHORTCUT: &str = "Fastctx.lnk";
#[cfg(debug_assertions)]
const SHORTCUT: &str = "Fastctx development.lnk";
#[cfg(not(debug_assertions))]
const ACTIVATOR: GUID = GUID::from_u128(0xef96ea86_9390_4a55_ba88_171c5d4bb83e);
#[cfg(debug_assertions)]
const ACTIVATOR: GUID = GUID::from_u128(0x74dd9b87_7c59_4ee3_9506_6f0406a472ce);

/// Windows' "the notification platform is unavailable" (`wpnapps.h`).
const WPN_E_PLATFORM_UNAVAILABLE: i32 = 0x803E_0105_u32 as i32;

const PROPERTY_SET: GUID = GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3);
const APP_ID_KEY: PROPERTYKEY = PROPERTYKEY { fmtid: PROPERTY_SET, pid: 5 };
const ACTIVATOR_KEY: PROPERTYKEY = PROPERTYKEY { fmtid: PROPERTY_SET, pid: 26 };

fn shortcut() -> Result<PathBuf, String> {
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None).map_err(|e| e.to_string())?;
        let result = path.to_string().map_err(|e| e.to_string());
        CoTaskMemFree(Some(path.0.cast()));
        Ok(PathBuf::from(result?).join(SHORTCUT))
    }
}

fn command() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(format!("\"{}\" --desktop-activate \"%1\"", exe.display()))
}

/// Creates a per-user protocol and a shortcut with both toast properties.
/// The CLSID is deliberately a stub: every toast activation is a protocol
/// activation, so no LocalServer32 or COM server is registered.
pub fn install() -> Result<(), String> {
    protocol_exists()?;
    let key = CURRENT_USER.create(format!("Software\\Classes\\{SCHEME}")).map_err(|e| e.to_string())?;
    key.set_string("", "URL:Fastctx notification").map_err(|e| e.to_string())?;
    key.set_string("URL Protocol", "").map_err(|e| e.to_string())?;
    key.create("shell\\open\\command")
        .map_err(|e| e.to_string())?
        .set_string("", command()?)
        .map_err(|e| e.to_string())?;
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.to_string())?;
        link.SetPath(&HSTRING::from(std::env::current_exe().map_err(|e| e.to_string())?.as_path()))
            .map_err(|e| e.to_string())?;
        let properties: IPropertyStore = link.cast().map_err(|e| e.to_string())?;
        let mut app_id = PROPVARIANT::default();
        PropVariantChangeType(&mut app_id, &PROPVARIANT::from(APP_ID), PVCHF_DEFAULT, VT_LPWSTR)
            .map_err(|e| e.to_string())?;
        properties.SetValue(&APP_ID_KEY, &app_id).map_err(|e| e.to_string())?;
        let activator = InitPropVariantFromCLSID(&ACTIVATOR).map_err(|e| e.to_string())?;
        properties.SetValue(&ACTIVATOR_KEY, &activator).map_err(|e| e.to_string())?;
        properties.Commit().map_err(|e| e.to_string())?;
        let file: IPersistFile = link.cast().map_err(|e| e.to_string())?;
        file.Save(&HSTRING::from(shortcut()?.as_path()), true).map_err(|e| e.to_string())?;
    }
    // The repair shortcut is a convenience beside the identity notifications
    // need; an installation goes on without it, and says why in its details.
    if let Err(error) = repair_link() {
        eprintln!("the Repair Fastctx shortcut was not created: {error}");
    }
    unsafe { shell_changed() }
}

/// A Start menu shortcut that opens this program's own repair.
fn repair_link() -> Result<(), String> {
    unsafe {
        let repair: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.to_string())?;
        repair
            .SetPath(&HSTRING::from(std::env::current_exe().map_err(|e| e.to_string())?.as_path()))
            .map_err(|e| e.to_string())?;
        repair.SetArguments(&HSTRING::from("--repair")).map_err(|e| e.to_string())?;
        repair
            .SetDescription(&HSTRING::from("Repair Fastctx program files; keep user data"))
            .map_err(|e| e.to_string())?;
        let file: IPersistFile = repair.cast().map_err(|e| e.to_string())?;
        file.Save(&HSTRING::from(repair_shortcut()?.as_path()), true).map_err(|e| e.to_string())
    }
}

/// Refuses to remove another installation's handler.
pub fn uninstall() -> Result<(), String> {
    let has_protocol = protocol_exists()?;
    let installed = registered_command()?;
    let expected = command()?;
    if installed.as_ref().is_some_and(|installed| installed != &expected) {
        return Err("Fastctx's protocol belongs to a different executable; refusing to remove it".into());
    }
    let shortcut = shortcut()?;
    let has_shortcut = shortcut.try_exists().map_err(|e| e.to_string())?;
    if has_shortcut {
        verify_shortcut()?;
    }
    match windows::UI::Notifications::ToastNotificationManager::History()
        .and_then(|history| history.ClearWithId(&HSTRING::from(APP_ID)))
    {
        Ok(()) => {}
        // A removal run where no one is signed in to a desktop (remotely, or
        // unattended) has no notification platform to ask. What World showed
        // stays in the history Windows keeps for this user, and is said.
        Err(error) if error.code().0 == WPN_E_PLATFORM_UNAVAILABLE => crate::installer_lines::say(&format!(
            "Left on this computer: notifications Fastctx showed stay in Windows' notification history, which cannot be reached from this session ({error})"
        )),
        Err(error) => return Err(error.to_string()),
    }
    if has_shortcut {
        std::fs::remove_file(shortcut).map_err(|e| e.to_string())?;
    }
    delete_repair_shortcut()?;
    if has_protocol {
        CURRENT_USER.remove_tree(format!("Software\\Classes\\{SCHEME}")).map_err(|e| e.to_string())?;
    }
    unsafe {
        shell_changed()?;
    }
    Ok(())
}

unsafe fn shell_changed() -> Result<(), String> {
    let shortcut = shortcut()?;
    let directory = HSTRING::from(shortcut.parent().expect("Start menu parent"));
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST | SHCNF_FLUSH, None, None);
        SHChangeNotify(SHCNE_UPDATEDIR, SHCNF_PATHW | SHCNF_FLUSH, Some(directory.as_ptr().cast()), None);
    }
    Ok(())
}

/// Sending must fail when this executable is not the installed handler.
pub fn verify() -> Result<(), String> {
    let installed = registered_command()?.ok_or("Fastctx notification identity is not installed")?;
    if installed != command()? {
        return Err("Fastctx notifications belong to a different executable; run the installed application".into());
    }
    verify_shortcut()
}

fn registered_command() -> Result<Option<String>, String> {
    match CURRENT_USER
        .open(format!("Software\\Classes\\{SCHEME}\\shell\\open\\command"))
        .and_then(|key| key.get_string(""))
    {
        Ok(command) => Ok(Some(command)),
        Err(error) if error.code().0 == 0x80070002u32 as i32 => Ok(None),
        Err(error) => Err(format!("reading Fastctx notification identity: {error}")),
    }
}

fn protocol_exists() -> Result<bool, String> {
    match CURRENT_USER.open(format!("Software\\Classes\\{SCHEME}")) {
        Ok(key) => {
            if key.get_string("").map_err(|e| e.to_string())? != "URL:Fastctx notification" {
                return Err("Fastctx's protocol name is registered by another application".into());
            }
            Ok(true)
        }
        Err(error) if error.code().0 == 0x80070002u32 as i32 => Ok(false),
        Err(error) => Err(format!("reading Fastctx protocol registration: {error}")),
    }
}

fn verify_shortcut() -> Result<(), String> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.to_string())?;
        let file: IPersistFile = link.cast().map_err(|e| e.to_string())?;
        file.Load(&HSTRING::from(shortcut()?.as_path()), windows::Win32::System::Com::STGM_READ)
            .map_err(|e| e.to_string())?;
        let properties: IPropertyStore = link.cast().map_err(|e| e.to_string())?;
        let actual = properties.GetValue(&APP_ID_KEY).map_err(|e| e.to_string())?;
        if BSTR::try_from(&actual).map_err(|e| e.to_string())? != APP_ID {
            return Err("Fastctx's Start menu shortcut has the wrong AppUserModelID".into());
        }
        let actual = properties.GetValue(&ACTIVATOR_KEY).map_err(|e| e.to_string())?;
        let expected = InitPropVariantFromCLSID(&ACTIVATOR).map_err(|e| e.to_string())?;
        if actual != expected {
            return Err("Fastctx's Start menu shortcut has the wrong toast activator".into());
        }
    }
    Ok(())
}

fn repair_shortcut() -> Result<PathBuf, String> {
    Ok(shortcut()?.with_file_name(if cfg!(debug_assertions) {
        "修复 Fastctx development.lnk"
    } else {
        "修复 Fastctx.lnk"
    }))
}

fn delete_repair_shortcut() -> Result<(), String> {
    let path = repair_shortcut()?;
    if !path.try_exists().map_err(|e| format!("checking {}: {e}", path.display()))? {
        return Ok(());
    }
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.to_string())?;
        let file: IPersistFile = link.cast().map_err(|e| e.to_string())?;
        file.Load(&HSTRING::from(path.as_path()), windows::Win32::System::Com::STGM_READ).map_err(|e| e.to_string())?;
        let mut target = vec![0u16; 32768];
        link.GetPath(&mut target, std::ptr::null_mut(), 0).map_err(|e| e.to_string())?;
        target.truncate(target.iter().position(|&c| c == 0).unwrap_or(target.len()));
        let target = PathBuf::from(String::from_utf16_lossy(&target));
        if target != std::env::current_exe().map_err(|e| e.to_string())? {
            return Err(format!(
                "repair shortcut {} belongs to {}; it was left in place",
                path.display(),
                target.display()
            ));
        }
    }
    std::fs::remove_file(&path).map_err(|e| format!("deleting repair shortcut {}: {e}", path.display()))
}
