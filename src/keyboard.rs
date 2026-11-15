//! On-screen keyboard: the Windows touch keyboard, with osk.exe as a fallback.

use std::mem::{size_of, zeroed};
use std::ptr::null;
use std::thread;
use std::time::{Duration, Instant};

use windows::core::{interface, IUnknown, IUnknown_Vtbl, GUID, HRESULT};
use windows::Win32::Foundation::{HWND as ComHwnd, REGDB_E_CLASSNOTREG};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::{FrameworkInputPane, IFrameworkInputPane};
use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::WaitForSingleObject;
use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::apps;
use crate::util::{from_wide, log, wide};

/// The touch keyboard's toggle, served by TabTip.exe; undocumented, so not in the SDK headers.
#[interface("37c994e7-432b-4834-a2f7-dce1f13b834b")]
unsafe trait ITipInvocation: IUnknown {
    fn toggle(&self, desktop: ComHwnd) -> HRESULT;
}

const CLSID_TIP_INVOCATION: GUID = GUID::from_u128(0x4ce576fa_83dc_4f88_951c_9d0782b4e376);

const OSK_CLASS: &str = "OSKMainClass";

/// How long a freshly started TabTip.exe gets to register ITipInvocation.
const TABTIP_START_TIMEOUT: Duration = Duration::from_secs(3);
const TABTIP_START_CHECK: u32 = 50;

/// The touch keyboard's on-screen rect is empty while it's hidden.
pub fn touch_keyboard_visible() -> bool {
    let pane: windows::core::Result<IFrameworkInputPane> = unsafe { CoCreateInstance(&FrameworkInputPane, None, CLSCTX_INPROC_SERVER) };
    pane.and_then(|pane| unsafe { pane.Location() }).is_ok_and(|r| r.right > r.left && r.bottom > r.top)
}

unsafe extern "system" fn find_classic_keyboard(h: HWND, found: LPARAM) -> BOOL {
    let mut class = [0u16; 64];
    let n = GetClassNameW(h, class.as_mut_ptr(), class.len() as i32);
    let visible = IsWindowVisible(h) != 0;
    let shown = match from_wide(&class, n.max(0) as usize).as_str() {
        "IPTip_Main_Window" => visible && (GetWindowLongW(h, GWL_STYLE) as u32 & WS_DISABLED) == 0,
        OSK_CLASS => visible,
        _ => false,
    };
    if shown {
        *(found as *mut bool) = true;
        return 0;
    }
    1
}

pub fn visible() -> bool {
    if touch_keyboard_visible() {
        return true;
    }
    let mut found = false;
    unsafe { EnumWindows(Some(find_classic_keyboard), &mut found as *mut bool as LPARAM) };
    found
}

/// Blocks while TabTip starts; from the UI thread use `toggle_async`.
pub fn toggle() {
    unsafe {
        let osk = FindWindowW(wide(OSK_CLASS).as_ptr(), null());
        if !osk.is_null() && IsWindowVisible(osk) != 0 {
            PostMessageW(osk, WM_SYSCOMMAND, SC_CLOSE as usize, 0);
            return;
        }
    }
    if !toggle_touch_keyboard() {
        apps::open("osk.exe");
    }
}

pub fn toggle_async() {
    on_worker(toggle);
}

/// Hides the touch keyboard; only call while it's showing.
pub fn hide_touch_keyboard_async() {
    on_worker(|| {
        toggle_touch_keyboard();
    });
}

fn on_worker(work: fn()) {
    thread::spawn(move || {
        let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if com.is_err() {
            log(&format!("touch keyboard: COM unavailable ({com:?})"));
            return;
        }
        work();
        unsafe { CoUninitialize() };
    });
}

fn toggle_touch_keyboard() -> bool {
    let Some(tip) = tip_invocation() else {
        return false;
    };
    let result = unsafe { tip.toggle(ComHwnd(GetDesktopWindow())) }.ok();
    if let Err(e) = &result {
        log(&format!("touch keyboard: Toggle failed ({e})"));
    }
    result.is_ok()
}

/// TabTip.exe is usually kept running by Windows; start it if it isn't.
fn tip_invocation() -> Option<ITipInvocation> {
    let create = || unsafe { CoCreateInstance::<_, ITipInvocation>(&CLSID_TIP_INVOCATION, None, CLSCTX_LOCAL_SERVER) };
    match create() {
        Ok(tip) => return Some(tip),
        Err(e) if e.code() == REGDB_E_CLASSNOTREG => {}
        Err(e) => {
            log(&format!("touch keyboard: ITipInvocation unavailable ({e})"));
            return None;
        }
    }

    let common = std::env::var("CommonProgramFiles").unwrap_or_else(|_| r"C:\Program Files\Common Files".into());
    let tabtip = wide(&format!(r"{common}\microsoft shared\ink\TabTip.exe"));
    let process = unsafe {
        let mut info: SHELLEXECUTEINFOW = zeroed();
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpFile = tabtip.as_ptr();
        info.nShow = SW_SHOWNORMAL;
        // TabTip has a uiAccess manifest, so it has to go through ShellExecute
        if ShellExecuteExW(&mut info) == 0 || info.hProcess.is_null() {
            log("touch keyboard: couldn't start TabTip.exe");
            return None;
        }
        info.hProcess
    };

    // TabTip can't say when its class is registered, so check until it is, it exits, or time runs out
    let deadline = Instant::now() + TABTIP_START_TIMEOUT;
    let tip = loop {
        let running = unsafe { WaitForSingleObject(process, TABTIP_START_CHECK) } == WAIT_TIMEOUT;
        match create() {
            Ok(tip) => break Some(tip),
            Err(e) if !running || Instant::now() >= deadline => {
                log(&format!("touch keyboard: TabTip.exe started but ITipInvocation is unavailable ({e})"));
                break None;
            }
            Err(_) => {}
        }
    };
    unsafe { CloseHandle(process) };
    tip
}
