//! The administrator step: Program Files, signing, the Start menu and Settings > Apps.

use std::fs;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT};
use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;
use windows_sys::Win32::UI::Shell::{FOLDERID_CommonPrograms, FOLDERID_ProgramFiles};

use crate::win::{self, wide, RegKey, Result};
use crate::{dialog, shortcut, signing, APP, APP_EXE, LINK_NAME, SHORTCUT_DESCRIPTION, TEMPORARY_UNINSTALLER};

static APP_BINARY: &[u8] = include_bytes!(env!("HOMEBAR_EXE"));

pub const INSTALL_SWITCH: &str = "--install-for-machine";
pub const UNINSTALL_SWITCH: &str = "--uninstall-for-machine";
const UNINSTALLER: &str = "uninstall.exe";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\HomeBar";

pub fn install_dir() -> Result<PathBuf> {
    Ok(win::known_folder(&FOLDERID_ProgramFiles)?.join(APP))
}

pub fn app_path() -> Result<PathBuf> {
    Ok(install_dir()?.join(APP_EXE))
}

fn start_menu_link() -> Result<PathBuf> {
    Ok(win::known_folder(&FOLDERID_CommonPrograms)?.join(LINK_NAME))
}

/// Exit code for the unelevated setup: 0 installed, 1 failed (already reported here).
pub fn install() -> u32 {
    match try_install() {
        Ok(()) => 0,
        Err(why) => {
            let undone = remove().map_or_else(|e| format!("\n\nUndoing the install also failed: {e}"), |()| String::new());
            dialog::error(&format!("Couldn't install {APP}"), &format!("{why}{undone}"));
            1
        }
    }
}

pub fn uninstall() -> u32 {
    match remove() {
        Ok(()) => {
            delete_self_if_temporary();
            0
        }
        Err(why) => {
            dialog::error(&format!("Couldn't uninstall {APP}"), &why);
            1
        }
    }
}

fn try_install() -> Result<()> {
    let dir = install_dir()?;
    let app = dir.join(APP_EXE);
    win::stop_app(&app);
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| format!("replacing {}: {e}", dir.display()))?;
    }
    fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    fs::write(&app, APP_BINARY).map_err(|e| format!("writing {}: {e}", app.display()))?;
    let setup = std::env::current_exe().map_err(|e| format!("finding the setup: {e}"))?;
    fs::copy(&setup, dir.join(UNINSTALLER)).map_err(|e| format!("copying the uninstaller: {e}"))?;
    signing::sign(&app)?;
    shortcut::create(&start_menu_link()?, &app, SHORTCUT_DESCRIPTION)?;
    register_uninstaller(&dir)
}

fn register_uninstaller(dir: &Path) -> Result<()> {
    let key = RegKey::create(HKEY_LOCAL_MACHINE, UNINSTALL_KEY)?;
    let size_kb = fs::read_dir(dir).map_err(|e| e.to_string())?.flatten().filter_map(|f| f.metadata().ok()).map(|m| m.len()).sum::<u64>() / 1024;
    key.set_string("DisplayName", APP)?;
    key.set_string("DisplayVersion", env!("CARGO_PKG_VERSION"))?;
    key.set_string("Publisher", APP)?;
    key.set_string("DisplayIcon", &format!("{},0", dir.join(APP_EXE).display()))?;
    key.set_string("InstallLocation", &dir.display().to_string())?;
    key.set_string("UninstallString", &format!("\"{}\" --uninstall", dir.join(UNINSTALLER).display()))?;
    key.set_dword("EstimatedSize", size_kb as u32)?;
    key.set_dword("NoModify", 1)?;
    key.set_dword("NoRepair", 1)
}

/// Removes everything this step adds; also used to undo a failed install.
fn remove() -> Result<()> {
    let dir = install_dir()?;
    win::stop_app(&dir.join(APP_EXE));
    signing::remove_certificates()?;
    win::remove_file(&start_menu_link()?)?;
    win::delete_key_tree(HKEY_LOCAL_MACHINE, UNINSTALL_KEY)?;
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| format!("removing {}: {e}", dir.display()))?;
    }
    Ok(())
}

/// The uninstaller runs from a copy in %TEMP% (see `crate::uninstall`); that copy goes at the next restart.
fn delete_self_if_temporary() {
    if let Ok(me) = std::env::current_exe() {
        if me.file_name().is_some_and(|name| name.eq_ignore_ascii_case(TEMPORARY_UNINSTALLER)) {
            unsafe { MoveFileExW(wide(&me).as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) };
        }
    }
}
