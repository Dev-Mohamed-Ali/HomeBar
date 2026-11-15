//! Desktop and Start menu shortcuts (.lnk files).

use std::path::Path;

use windows::core::{Interface, HSTRING};
use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

use crate::win::Result;

/// Writes a .lnk at `link` that starts `target`, using the target's own icon.
pub fn create(link: &Path, target: &Path, description: &str) -> Result<()> {
    let write = || -> windows::core::Result<()> {
        unsafe {
            let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            let target = HSTRING::from(target);
            shell_link.SetPath(&target)?;
            shell_link.SetDescription(&HSTRING::from(description))?;
            shell_link.SetIconLocation(&target, 0)?;
            shell_link.cast::<IPersistFile>()?.Save(&HSTRING::from(link), true)
        }
    };
    write().map_err(|e| format!("creating the shortcut {}: {e}", link.display()))
}
