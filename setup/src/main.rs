//! HomeBar-Setup.exe: installs HomeBar, and uninstalls it from Settings > Apps.
#![windows_subsystem = "windows"]

mod dialog;
mod machine;
mod shortcut;
mod signing;
mod user;
mod win;

use std::path::{Path, PathBuf};
use std::ptr::null;

use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};

use win::Elevated;

pub const APP: &str = "HomeBar";
pub const APP_EXE: &str = "HomeBar.exe";
pub const LINK_NAME: &str = "HomeBar.lnk";
pub const SHORTCUT_DESCRIPTION: &str = "Start HomeBar, then press Home on your controller";
pub const TEMPORARY_UNINSTALLER: &str = "HomeBar-uninstall.exe";

fn main() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32);
    }
    let code = match std::env::args().nth(1).as_deref() {
        None => install(),
        Some("--uninstall") => uninstall(),
        Some(machine::INSTALL_SWITCH) => machine::install(),
        Some(machine::UNINSTALL_SWITCH) => machine::uninstall(),
        Some(other) => {
            dialog::error("Unknown option", &format!("{other}\n\nRun HomeBar-Setup.exe without options to install, or with --uninstall."));
            2
        }
    };
    std::process::exit(code as i32);
}

fn setup_path() -> Option<PathBuf> {
    std::env::current_exe().map_err(|e| dialog::error("Couldn't start the setup", &e.to_string())).ok()
}

/// Runs this exe's administrator step; true when it succeeded (it reports its own failures).
fn run_machine_step(setup: &Path, switch: &str, failed: &str) -> bool {
    match win::run_elevated(setup, switch) {
        Ok(Elevated::Finished(0)) => true,
        Ok(Elevated::Finished(_)) | Ok(Elevated::Declined) => false,
        Err(why) => {
            dialog::error(failed, &why);
            false
        }
    }
}

fn install() -> u32 {
    let Some(setup) = setup_path() else { return 1 };
    let app = match machine::app_path() {
        Ok(app) => app,
        Err(why) => {
            dialog::error(&format!("Couldn't install {APP}"), &why);
            return 1;
        }
    };
    let installed = app.exists();
    let Some(start_with_windows) = dialog::confirm_install(installed, !installed || user::starts_with_windows()) else { return 0 };
    if !run_machine_step(&setup, machine::INSTALL_SWITCH, &format!("Couldn't install {APP}")) {
        return 1;
    }
    if let Err(why) = user::install(&app, start_with_windows) {
        dialog::error(&format!("{APP} is installed, but a shortcut or startup setting failed"), &why);
    }
    if let Err(why) = win::shell_open(&app, "") {
        dialog::error(&format!("{APP} is installed but didn't start"), &format!("{why}

Start it from the desktop shortcut."));
        return 1;
    }
    dialog::done(&format!("{APP} is ready"), "Press the Home button on your controller to open it.");
    0
}

fn uninstall() -> u32 {
    let Some(setup) = setup_path() else { return 1 };
    // the copy in Program Files can't delete its own folder, so hand over to a copy in %TEMP%
    if machine::install_dir().is_ok_and(|dir| setup.starts_with(dir)) {
        let temporary = std::env::temp_dir().join(TEMPORARY_UNINSTALLER);
        if let Err(e) = std::fs::copy(&setup, &temporary) {
            dialog::error(&format!("Couldn't uninstall {APP}"), &format!("copying the uninstaller to {}: {e}", temporary.display()));
            return 1;
        }
        if let Err(why) = win::shell_open(&temporary, "--uninstall") {
            dialog::error(&format!("Couldn't uninstall {APP}"), &why);
            return 1;
        }
        return 0;
    }
    if !dialog::confirm_uninstall() {
        return 0;
    }
    if !run_machine_step(&setup, machine::UNINSTALL_SWITCH, &format!("Couldn't uninstall {APP}")) {
        return 1;
    }
    if let Err(why) = user::uninstall() {
        dialog::error(&format!("{APP} is uninstalled, but some of its settings couldn't be removed"), &why);
        return 1;
    }
    dialog::done(&format!("{APP} is uninstalled"), "Everything it added has been removed.");
    0
}
