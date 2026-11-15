//! "Start with Windows": the per-user Run key, honouring the switch in Task Manager > Startup apps.

use std::ffi::c_void;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows_sys::Win32::System::Registry::*;

use crate::util::{log, wide, APP_NAME};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Task Manager's on/off state per Run entry; an odd first byte means turned off there.
const APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

fn command() -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    format!("\"{}\"", exe.display())
}

fn value_size(key: &str) -> Option<u32> {
    let mut size = 0u32;
    let status = unsafe { RegGetValueW(HKEY_CURRENT_USER, wide(key).as_ptr(), wide(APP_NAME).as_ptr(), RRF_RT_ANY, null_mut(), null_mut(), &mut size) };
    (status == 0).then_some(size)
}

pub fn is_enabled() -> bool {
    if value_size(RUN_KEY).is_none() {
        return false;
    }
    let mut state = [0u8; 12];
    let mut size = state.len() as u32;
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, wide(APPROVED_KEY).as_ptr(), wide(APP_NAME).as_ptr(), RRF_RT_REG_BINARY, null_mut(), state.as_mut_ptr() as *mut c_void, &mut size)
    };
    status != 0 || size == 0 || state[0] & 1 == 0
}

pub fn set_enabled(enable: bool) -> bool {
    let name = wide(APP_NAME);
    let status = unsafe {
        // a stale Task Manager "off" would otherwise override the user's choice here
        RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(APPROVED_KEY).as_ptr(), name.as_ptr());
        if enable {
            let command = wide(&command());
            RegSetKeyValueW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), name.as_ptr(), REG_SZ, command.as_ptr() as *const c_void, (command.len() * 2) as u32)
        } else {
            match RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), name.as_ptr()) {
                ERROR_FILE_NOT_FOUND => 0,
                status => status,
            }
        }
    };
    if status != 0 {
        log(&format!("startup: turning {} failed ({status})", if enable { "on" } else { "off" }));
    }
    status == 0
}
