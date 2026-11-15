//! Per-user parts, done as the signed-in user rather than the administrator: the desktop
//! shortcut, Start with Windows, and the settings HomeBar keeps under HKEY_CURRENT_USER.

use std::path::Path;

use windows_sys::Win32::System::Registry::HKEY_CURRENT_USER;
use windows_sys::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Programs, FOLDERID_Startup};

use crate::win::{self, RegKey, Result};
use crate::{shortcut, APP, LINK_NAME, SHORTCUT_DESCRIPTION};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const APP_KEY: &str = r"Software\HomeBar";
const GAME_BAR_KEY: &str = r"Software\Microsoft\GameBar";
const TRAY_SETTINGS_KEY: &str = r"Control Panel\NotifyIconSettings";

pub fn starts_with_windows() -> bool {
    RegKey::open(HKEY_CURRENT_USER, RUN_KEY).is_some_and(|run| run.has_value(APP))
}

pub fn install(app: &Path, start_with_windows: bool) -> Result<()> {
    remove_old_shortcuts()?;
    shortcut::create(&win::known_folder(&FOLDERID_Desktop)?.join(LINK_NAME), app, SHORTCUT_DESCRIPTION)?;
    set_start_with_windows(start_with_windows.then_some(app))
}

pub fn uninstall() -> Result<()> {
    remove_old_shortcuts()?;
    win::remove_file(&win::known_folder(&FOLDERID_Desktop)?.join(LINK_NAME))?;
    set_start_with_windows(None)?;
    restore_game_bar()?;
    win::delete_key_tree(HKEY_CURRENT_USER, APP_KEY)?;
    forget_tray_icons()?;
    win::remove_file(&std::env::temp_dir().join("HomeBar.log"))
}

/// The PowerShell installer put shortcuts in Startup and the per-user Start menu.
fn remove_old_shortcuts() -> Result<()> {
    for folder in [FOLDERID_Startup, FOLDERID_Programs] {
        win::remove_file(&win::known_folder(&folder)?.join(LINK_NAME))?;
    }
    Ok(())
}

/// Same Run entry as the tray menu's "Start with Windows"; `None` turns it off.
fn set_start_with_windows(app: Option<&Path>) -> Result<()> {
    // clears an "off" left in Task Manager, which would override this choice
    RegKey::create(HKEY_CURRENT_USER, APPROVED_KEY)?.delete_value(APP)?;
    let run = RegKey::create(HKEY_CURRENT_USER, RUN_KEY)?;
    match app {
        Some(app) => run.set_string(APP, &format!("\"{}\"", app.display())),
        None => run.delete_value(APP),
    }
}

/// Turns Game Bar's Home-button shortcut back on, only if HomeBar turned it off.
fn restore_game_bar() -> Result<()> {
    let turned_off = RegKey::open(HKEY_CURRENT_USER, APP_KEY).and_then(|k| k.dword("GameBarTurnedOff")) == Some(1);
    if turned_off {
        RegKey::create(HKEY_CURRENT_USER, GAME_BAR_KEY)?.set_dword("UseNexusForGameBarEnabled", 1)?;
    }
    Ok(())
}

/// Explorer's remembered tray icon entries for HomeBar.exe.
fn forget_tray_icons() -> Result<()> {
    let Some(tray) = RegKey::open(HKEY_CURRENT_USER, TRAY_SETTINGS_KEY) else { return Ok(()) };
    for entry in tray.subkeys() {
        let path = tray.string(&entry, "ExecutablePath").unwrap_or_default();
        if path.to_ascii_lowercase().ends_with(r"\homebar.exe") {
            win::delete_key_tree(HKEY_CURRENT_USER, &format!(r"{TRAY_SETTINGS_KEY}\{entry}"))?;
        }
    }
    Ok(())
}
