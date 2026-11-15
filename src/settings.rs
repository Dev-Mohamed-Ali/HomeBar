//! Per-user registry settings: Game Bar's Home-button shortcut, tray icon visibility, taskbar theme.

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::util::{from_wide, log, message_box, wide};

const APP_KEY: &str = r"Software\HomeBar";
const GAME_BAR_KEY: &str = r"Software\Microsoft\GameBar";
const GAME_BAR_HOME_BUTTON: &str = "UseNexusForGameBarEnabled";
const KEPT_GAME_BAR: &str = "GameBarKept";
/// Set when HomeBar turned the shortcut off, so uninstalling turns it back on.
const TURNED_OFF_GAME_BAR: &str = "GameBarTurnedOff";
const TRAY_SETTINGS_KEY: &str = r"Control Panel\NotifyIconSettings";

fn read_dword(key: &str, name: &str) -> Option<u32> {
    read_dword_at(HKEY_CURRENT_USER, &wide(key), name)
}

/// `subkey` is NUL-terminated UTF-16.
fn read_dword_at(root: HKEY, subkey: &[u16], name: &str) -> Option<u32> {
    let mut value = 0u32;
    let mut size = 4u32;
    let status = unsafe { RegGetValueW(root, subkey.as_ptr(), wide(name).as_ptr(), RRF_RT_REG_DWORD, null_mut(), &mut value as *mut u32 as *mut c_void, &mut size) };
    (status == 0).then_some(value)
}

fn write_dword(key: &str, name: &str, value: u32) -> bool {
    let status = unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, wide(key).as_ptr(), wide(name).as_ptr(), REG_DWORD, &value as *const u32 as *const c_void, 4) };
    if status != 0 {
        log(&format!(r"settings: writing HKCU\{key}\{name} failed ({status})"));
    }
    status == 0
}

pub fn taskbar_is_light() -> bool {
    read_dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "SystemUsesLightTheme") == Some(1)
}

/// Game Bar opens on the Home button too. If that shortcut is on, ask once whether to turn it off
/// (Settings > Gaming > Xbox Game Bar > "Allow your controller to open Game Bar"). Blocks; run off the UI thread.
pub fn offer_to_turn_off_game_bar_shortcut() {
    let shortcut_on = read_dword(GAME_BAR_KEY, GAME_BAR_HOME_BUTTON).unwrap_or(1) != 0; // missing means on
    if !shortcut_on || read_dword(APP_KEY, KEPT_GAME_BAR) == Some(1) {
        return;
    }
    let question = "Xbox Game Bar also opens when you press the Home button on your controller, so both would pop up.\n\n\
                    Turn off Game Bar's Home-button shortcut?\n\n\
                    Game Bar and Win+G keep working. You can turn it back on in Settings > Gaming > Xbox Game Bar.";
    if message_box(question, MB_YESNO | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND) == IDYES {
        if write_dword(GAME_BAR_KEY, GAME_BAR_HOME_BUTTON, 0) {
            write_dword(APP_KEY, TURNED_OFF_GAME_BAR, 1);
            log("Game Bar Home-button shortcut turned off (user agreed)");
        }
    } else {
        write_dword(APP_KEY, KEPT_GAME_BAR, 1);
    }
}

/// Windows 11 puts new tray icons in the overflow. Show ours, unless the user already chose
/// (an existing IsPromoted value is left alone). Returns the icon IDs Explorer has recorded so far.
pub fn show_tray_icons() -> Vec<u32> {
    let mut found = Vec::new();
    unsafe {
        let mut key: HKEY = null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, wide(TRAY_SETTINGS_KEY).as_ptr(), 0, KEY_READ | KEY_SET_VALUE, &mut key) != 0 {
            return found;
        }
        let promoted = wide("IsPromoted");
        let mut index = 0;
        loop {
            let mut name = [0u16; 64];
            let mut name_len = name.len() as u32;
            if RegEnumKeyExW(key, index, name.as_mut_ptr(), &mut name_len, null(), null_mut(), null_mut(), null_mut()) != 0 {
                break;
            }
            index += 1;

            let mut path = [0u16; 520];
            let mut size = (path.len() * 2) as u32;
            let read = RegGetValueW(key, name.as_ptr(), wide("ExecutablePath").as_ptr(), RRF_RT_REG_SZ, null_mut(), path.as_mut_ptr() as *mut c_void, &mut size);
            if read != 0 || !from_wide(&path, path.len()).to_ascii_lowercase().ends_with("\\homebar.exe") {
                continue;
            }
            if let Some(uid) = read_dword_at(key, &name, "UID") {
                found.push(uid);
            }
            if read_dword_at(key, &name, "IsPromoted").is_none() {
                let one = 1u32;
                let status = RegSetKeyValueW(key, name.as_ptr(), promoted.as_ptr(), REG_DWORD, &one as *const u32 as *const c_void, 4);
                if status != 0 {
                    log(&format!("settings: promoting the tray icon failed ({status})"));
                }
            }
        }
        RegCloseKey(key);
    }
    found
}
