//! The setup's windows: Task Dialogs with the HomeBar icon.

use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use windows_sys::core::PCWSTR;
use windows_sys::core::BOOL;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{IDCANCEL, IDOK};

use crate::win::wide;
use crate::APP;

const TITLE: &str = "HomeBar Setup";
const APP_ICON: PCWSTR = 1 as PCWSTR;

struct Dialog<'a> {
    icon: PCWSTR,
    heading: &'a str,
    text: &'a str,
    action: Option<&'a str>,
    checkbox: Option<(&'a str, bool)>,
}

impl Dialog<'_> {
    /// Returns whether the action button was chosen, and the checkbox state.
    fn show(&self) -> (bool, bool) {
        let title = wide(TITLE);
        let heading = wide(self.heading);
        let text = wide(self.text);
        let action = self.action.map(wide);
        let checkbox = self.checkbox.map(|(label, _)| wide(label));
        let buttons: Vec<TASKDIALOG_BUTTON> = action.iter().map(|label| TASKDIALOG_BUTTON { nButtonID: IDOK, pszButtonText: label.as_ptr() }).collect();
        unsafe {
            let mut config: TASKDIALOGCONFIG = zeroed();
            config.cbSize = size_of::<TASKDIALOGCONFIG>() as u32;
            config.hInstance = GetModuleHandleW(null());
            config.dwFlags = TDF_ALLOW_DIALOG_CANCELLATION | TDF_SIZE_TO_CONTENT;
            if self.checkbox.is_some_and(|(_, checked)| checked) {
                config.dwFlags |= TDF_VERIFICATION_FLAG_CHECKED;
            }
            config.dwCommonButtons = if action.is_some() { TDCBF_CANCEL_BUTTON } else { TDCBF_OK_BUTTON };
            config.pszWindowTitle = title.as_ptr();
            config.Anonymous1.pszMainIcon = self.icon;
            config.pszMainInstruction = heading.as_ptr();
            config.pszContent = text.as_ptr();
            config.cButtons = buttons.len() as u32;
            config.pButtons = buttons.as_ptr();
            config.nDefaultButton = IDOK;
            config.pszVerificationText = checkbox.as_ref().map_or(null(), |c| c.as_ptr());
            let mut button = IDCANCEL;
            let mut checked: BOOL = 0;
            if TaskDialogIndirect(&config, &mut button, null_mut(), &mut checked) < 0 {
                return (false, false);
            }
            (button == IDOK, checked != 0)
        }
    }
}

/// Asks to install; Some(start with Windows) to go ahead.
pub fn confirm_install(installed: bool, start_with_windows: bool) -> Option<bool> {
    let version = env!("CARGO_PKG_VERSION");
    let heading = if installed { format!("Update {APP} to {version}?") } else { format!("Install {APP} {version}?") };
    let (install, start) = Dialog {
        icon: APP_ICON,
        heading: &heading,
        text: "Press the Home button on your controller to open a menu with a mouse, keyboard, sound, apps and power.\n\n\
               Windows will ask for permission: HomeBar is installed in Program Files and signed for this PC, \
               so its menu can show above every other window.",
        action: Some(if installed { "Update" } else { "Install" }),
        checkbox: Some(("Start HomeBar with Windows", start_with_windows)),
    }
    .show();
    install.then_some(start)
}

pub fn confirm_uninstall() -> bool {
    Dialog {
        icon: APP_ICON,
        heading: &format!("Uninstall {APP}?"),
        text: "Removes HomeBar, its shortcuts, settings and signing certificate.",
        action: Some("Uninstall"),
        checkbox: None,
    }
    .show()
    .0
}

pub fn done(heading: &str, text: &str) {
    Dialog { icon: APP_ICON, heading, text, action: None, checkbox: None }.show();
}

pub fn error(heading: &str, detail: &str) {
    Dialog { icon: TD_ERROR_ICON, heading, text: detail, action: None, checkbox: None }.show();
}
