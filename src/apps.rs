//! Processes and top-level windows: launching, switching, closing, power.

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::{IShellDispatch4, Shell};
use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_INSUFFICIENT_BUFFER, HWND, INVALID_HANDLE_VALUE, LPARAM};
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
use windows_sys::Win32::System::Power::SetSuspendState;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::{state, util::{from_wide, log, wide}};

const VOICE_ACCESS: &str = "VoiceAccess.exe";
/// Floating helpers with their own menu rows; Switch app and Close app leave them out.
const HELPERS: [&str; 3] = ["voiceaccess.exe", "osk.exe", "textinputhost.exe"];
const SHELL_CLASSES: [&str; 4] = ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"];

pub fn open(target: &str) {
    shell_execute(target, None, SW_SHOWNORMAL);
}

fn shell_execute(file: &str, args: Option<&str>, show: SHOW_WINDOW_CMD) {
    let args = args.map(wide);
    unsafe {
        ShellExecuteW(null_mut(), wide("open").as_ptr(), wide(file).as_ptr(), args.as_ref().map_or(null(), |a| a.as_ptr()), null(), show);
    }
}

fn find_process(exe: &str) -> Option<u32> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut entry: PROCESSENTRY32W = zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut found = None;
        let mut more = Process32FirstW(snapshot, &mut entry);
        while more != 0 {
            if from_wide(&entry.szExeFile, entry.szExeFile.len()).eq_ignore_ascii_case(exe) {
                found = Some(entry.th32ProcessID);
                break;
            }
            more = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        found
    }
}

fn window_pid(w: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(w, &mut pid) };
    pid
}

/// File name of the process that owns `w`, e.g. "notepad.exe".
pub fn process_exe(w: HWND) -> String {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, window_pid(w));
        if process.is_null() {
            return String::new();
        }
        let mut path = vec![0u16; 260];
        let found = loop {
            let mut len = path.len() as u32;
            if QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut len) != 0 {
                break Some(len as usize);
            }
            if GetLastError() != ERROR_INSUFFICIENT_BUFFER || path.len() >= 32768 {
                break None;
            }
            path.resize(path.len() * 2, 0);
        };
        CloseHandle(process);
        found.map_or_else(String::new, |len| from_wide(&path, len).rsplit('\\').next().unwrap_or_default().to_string())
    }
}

fn terminate(pid: u32) {
    unsafe {
        let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !process.is_null() {
            TerminateProcess(process, 1);
            CloseHandle(process);
        }
    }
}

fn is_helper(w: HWND) -> bool {
    HELPERS.contains(&process_exe(w).to_ascii_lowercase().as_str())
}

/// Store apps share one ApplicationFrameHost process; killing it would close all of them.
pub fn is_shared_host(w: HWND) -> bool {
    process_exe(w).eq_ignore_ascii_case("ApplicationFrameHost.exe")
}

pub fn window_title(w: HWND) -> String {
    let mut title = [0u16; 256];
    let n = unsafe { GetWindowTextW(w, title.as_mut_ptr(), title.len() as i32) };
    from_wide(&title, n.max(0) as usize)
}

fn class_name(w: HWND) -> String {
    let mut class = [0u16; 64];
    let n = unsafe { GetClassNameW(w, class.as_mut_ptr(), class.len() as i32) };
    from_wide(&class, n.max(0) as usize)
}

pub fn voice_access_running() -> bool {
    find_process(VOICE_ACCESS).is_some()
}

pub fn toggle_voice_access() {
    match find_process(VOICE_ACCESS) {
        Some(pid) => terminate(pid),
        // voiceaccess.exe is a uiAccess app: ShellExecute works, CreateProcess doesn't
        None => open(&format!(r"{}\System32\voiceaccess.exe", std::env::var("windir").unwrap_or_else(|_| r"C:\Windows".into()))),
    }
}

/// The app behind `foreground` that Close app would act on, with its title.
/// None over the desktop, the taskbar, this menu or a helper like Voice Access.
pub fn close_target(foreground: HWND) -> Option<(HWND, String)> {
    if foreground.is_null() {
        return None;
    }
    let root = unsafe { GetAncestor(foreground, GA_ROOTOWNER) };
    if root == state::menu_window() || SHELL_CLASSES.contains(&class_name(root).as_str()) || is_helper(root) {
        return None;
    }
    let title = window_title(root);
    Some((root, if title.is_empty() { "this app".into() } else { title }))
}

unsafe extern "system" fn collect_app(w: HWND, list: LPARAM) -> BOOL {
    let apps = &mut *(list as *mut Vec<(HWND, String)>);
    let ex_style = GetWindowLongW(w, GWL_EXSTYLE) as u32;
    let mut cloaked = 0u32;
    DwmGetWindowAttribute(w, DWMWA_CLOAKED as u32, &mut cloaked as *mut u32 as *mut c_void, 4);
    let candidate = IsWindowVisible(w) != 0
        && GetWindow(w, GW_OWNER).is_null()
        && w != state::menu_window()
        && ex_style & (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) == 0
        && cloaked == 0
        && !is_helper(w);
    if candidate {
        let title = window_title(w);
        if !title.is_empty() && title != "Program Manager" {
            apps.push((w, title));
        }
    }
    1
}

/// Alt+Tab-style list of app windows, front-most first.
pub fn open_apps() -> Vec<(HWND, String)> {
    let mut apps = Vec::new();
    unsafe { EnumWindows(Some(collect_app), &mut apps as *mut _ as LPARAM) };
    apps
}

/// Restores `w` if minimised and gives it focus.
pub fn activate(w: HWND) {
    unsafe {
        if IsIconic(w) != 0 {
            ShowWindow(w, SW_RESTORE);
        }
        // only the foreground thread may hand over focus, so share its input queue briefly
        let foreground_thread = GetWindowThreadProcessId(GetForegroundWindow(), null_mut());
        let me = GetCurrentThreadId();
        let attached = foreground_thread != 0 && foreground_thread != me && AttachThreadInput(me, foreground_thread, 1) != 0;
        SetForegroundWindow(w);
        BringWindowToTop(w);
        if attached {
            AttachThreadInput(me, foreground_thread, 0);
        }
    }
}

/// Asks the window to close, as if its X were clicked.
pub fn request_close(w: HWND) {
    unsafe { PostMessageW(w, WM_CLOSE, 0, 0) };
}

pub fn force_close(w: HWND) {
    terminate(window_pid(w));
}

/// A visible dialog the window is waiting on, such as "Save changes?".
pub fn pending_dialog(w: HWND) -> Option<HWND> {
    let popup = unsafe { GetLastActivePopup(w) };
    (popup != w && unsafe { IsWindowVisible(popup) } != 0).then_some(popup)
}

pub fn still_open(w: HWND) -> bool {
    unsafe { IsWindow(w) != 0 && IsWindowVisible(w) != 0 }
}

pub fn sleep() {
    unsafe { SetSuspendState(false, false, false) };
}

pub fn restart() {
    shell_execute("shutdown.exe", Some("/r /t 0"), SW_HIDE);
}

pub fn shut_down() {
    shell_execute("shutdown.exe", Some("/s /t 0"), SW_HIDE);
}

/// What the taskbar's show-desktop corner does.
pub fn toggle_desktop() {
    let shell: windows::core::Result<IShellDispatch4> = unsafe { CoCreateInstance(&Shell, None, CLSCTX_INPROC_SERVER) };
    if let Err(e) = shell.and_then(|shell| unsafe { shell.ToggleDesktop() }) {
        log(&format!("show desktop failed ({e})"));
    }
}
