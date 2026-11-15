//! Thin wrappers over the Win32 calls the setup needs.

use std::ffi::{c_void, OsStr, OsString};
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::core::{GUID, HRESULT};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

pub type Result<T> = std::result::Result<T, String>;

/// NUL-terminated UTF-16 for Win32 `PCWSTR` parameters.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

/// "what: system message" for a Win32 error code or HRESULT.
pub fn os_error(what: &str, code: i32) -> String {
    format!("{what}: {}", std::io::Error::from_raw_os_error(code))
}

pub fn last_error(what: &str) -> String {
    os_error(what, unsafe { GetLastError() } as i32)
}

pub fn check(what: &str, hr: HRESULT) -> Result<()> {
    if hr < 0 { Err(os_error(what, hr)) } else { Ok(()) }
}

pub fn known_folder(id: &GUID) -> Result<PathBuf> {
    unsafe {
        let mut path = null_mut();
        let hr = SHGetKnownFolderPath(id, 0, null_mut(), &mut path);
        if hr < 0 {
            CoTaskMemFree(path as *const c_void);
            return Err(os_error("finding a system folder", hr));
        }
        let len = (0..).take_while(|&i| *path.add(i) != 0).count();
        let folder = OsString::from_wide(std::slice::from_raw_parts(path, len));
        CoTaskMemFree(path as *const c_void);
        Ok(folder.into())
    }
}

pub fn remove_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("removing {}: {e}", path.display())),
        _ => Ok(()),
    }
}

/// Starts `file` the way Explorer would, as the current user.
pub fn shell_open(file: &Path, args: &str) -> Result<()> {
    let file_w = wide(file);
    let args = wide(args);
    unsafe {
        let mut info: SHELLEXECUTEINFOW = zeroed();
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
        info.lpFile = file_w.as_ptr();
        info.lpParameters = args.as_ptr();
        info.nShow = SW_SHOWNORMAL;
        if ShellExecuteExW(&mut info) == 0 {
            return Err(last_error(&format!("starting {}", file.display())));
        }
    }
    Ok(())
}

pub enum Elevated {
    Finished(u32),
    Declined,
}

/// Runs `exe args` as administrator (the UAC prompt) and waits for its exit code.
pub fn run_elevated(exe: &Path, args: &str) -> Result<Elevated> {
    let verb = wide("runas");
    let file = wide(exe);
    let params = wide(args);
    unsafe {
        let mut info: SHELLEXECUTEINFOW = zeroed();
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_SHOWNORMAL;
        if ShellExecuteExW(&mut info) == 0 {
            return match GetLastError() {
                ERROR_CANCELLED => Ok(Elevated::Declined),
                code => Err(os_error("starting the administrator step", code as i32)),
            };
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 0;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        Ok(Elevated::Finished(code))
    }
}

fn process_ids(exe_name: &str) -> Vec<u32> {
    let mut ids = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return ids;
        }
        let mut entry: PROCESSENTRY32W = zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snapshot, &mut entry);
        while more != 0 {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..len]).eq_ignore_ascii_case(exe_name) {
                ids.push(entry.th32ProcessID);
            }
            more = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
    }
    ids
}

/// Asks a running HomeBar to quit (it releases held buttons and removes its tray icons),
/// then ends any copy still running after a few seconds.
pub fn stop_app(exe: &Path) {
    let name = exe.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let running = process_ids(&name);
    if running.is_empty() {
        return;
    }
    if exe.exists() {
        // HomeBar runs with uiAccess, so only another HomeBar process may message it
        let file = wide(exe);
        let params = wide("--quit");
        unsafe {
            let mut info: SHELLEXECUTEINFOW = zeroed();
            info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
            info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
            info.lpFile = file.as_ptr();
            info.lpParameters = params.as_ptr();
            if ShellExecuteExW(&mut info) != 0 && !info.hProcess.is_null() {
                WaitForSingleObject(info.hProcess, 5000);
                CloseHandle(info.hProcess);
            }
        }
    }
    for pid in running {
        unsafe {
            let process = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid);
            if process.is_null() {
                continue;
            }
            if WaitForSingleObject(process, 3000) == WAIT_TIMEOUT {
                TerminateProcess(process, 1);
                WaitForSingleObject(process, 3000);
            }
            CloseHandle(process);
        }
    }
}

pub struct RegKey(HKEY);

impl RegKey {
    pub fn create(root: HKEY, path: &str) -> Result<Self> {
        let mut key = null_mut();
        let status = unsafe { RegCreateKeyExW(root, wide(path).as_ptr(), 0, null(), 0, KEY_READ | KEY_WRITE, null(), &mut key, null_mut()) };
        if status != 0 {
            return Err(os_error(&format!(r"opening registry key {path}"), status as i32));
        }
        Ok(RegKey(key))
    }

    pub fn open(root: HKEY, path: &str) -> Option<Self> {
        let mut key = null_mut();
        let status = unsafe { RegOpenKeyExW(root, wide(path).as_ptr(), 0, KEY_READ | KEY_WRITE, &mut key) };
        (status == 0).then_some(RegKey(key))
    }

    fn set(&self, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> Result<()> {
        let status = unsafe { RegSetValueExW(self.0, wide(name).as_ptr(), 0, kind, data.as_ptr(), data.len() as u32) };
        if status != 0 {
            return Err(os_error(&format!("writing registry value {name}"), status as i32));
        }
        Ok(())
    }

    pub fn set_string(&self, name: &str, value: &str) -> Result<()> {
        let value = wide(value);
        let bytes = unsafe { std::slice::from_raw_parts(value.as_ptr() as *const u8, value.len() * 2) };
        self.set(name, REG_SZ, bytes)
    }

    pub fn set_dword(&self, name: &str, value: u32) -> Result<()> {
        self.set(name, REG_DWORD, &value.to_le_bytes())
    }

    pub fn dword(&self, name: &str) -> Option<u32> {
        let mut value = 0u32;
        let mut size = 4u32;
        let status = unsafe { RegGetValueW(self.0, null(), wide(name).as_ptr(), RRF_RT_REG_DWORD, null_mut(), &mut value as *mut u32 as *mut c_void, &mut size) };
        (status == 0).then_some(value)
    }

    pub fn string(&self, subkey: &str, name: &str) -> Option<String> {
        let mut buf = [0u16; 1024];
        let mut size = (buf.len() * 2) as u32;
        let status = unsafe { RegGetValueW(self.0, wide(subkey).as_ptr(), wide(name).as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr() as *mut c_void, &mut size) };
        if status != 0 {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }

    pub fn has_value(&self, name: &str) -> bool {
        unsafe { RegGetValueW(self.0, null(), wide(name).as_ptr(), RRF_RT_ANY, null_mut(), null_mut(), null_mut()) == 0 }
    }

    pub fn delete_value(&self, name: &str) -> Result<()> {
        match unsafe { RegDeleteValueW(self.0, wide(name).as_ptr()) } {
            0 | ERROR_FILE_NOT_FOUND => Ok(()),
            status => Err(os_error(&format!("removing registry value {name}"), status as i32)),
        }
    }

    pub fn subkeys(&self) -> Vec<String> {
        let mut names = Vec::new();
        for index in 0.. {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            if unsafe { RegEnumKeyExW(self.0, index, name.as_mut_ptr(), &mut len, null(), null_mut(), null_mut(), null_mut()) } != 0 {
                break;
            }
            names.push(String::from_utf16_lossy(&name[..len as usize]));
        }
        names
    }
}

impl Drop for RegKey {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

/// Deletes `path` and everything under it; a missing key is fine.
pub fn delete_key_tree(root: HKEY, path: &str) -> Result<()> {
    match unsafe { RegDeleteTreeW(root, wide(path).as_ptr()) } {
        0 | ERROR_FILE_NOT_FOUND => {}
        status => return Err(os_error(&format!(r"removing registry key {path}"), status as i32)),
    }
    match unsafe { RegDeleteKeyW(root, wide(path).as_ptr()) } {
        0 | ERROR_FILE_NOT_FOUND => Ok(()),
        status => Err(os_error(&format!(r"removing registry key {path}"), status as i32)),
    }
}
