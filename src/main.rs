#![windows_subsystem = "windows"]

mod apps;
mod audio;
mod capture;
mod input;
mod keyboard;
mod menu;
mod pad;
mod settings;
mod startup;
mod state;
mod tray;
mod update;
mod util;

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::thread;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::*;
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use pad::{PadEvent, XInput, WM_PAD};
use util::{message_box, wide};

const MENU_CLASS: &str = "HomeBarWnd";
const TRAY_OWNER_CLASS: &str = "HomeBarTrayOwner";

const USAGE: &str = "Usage: HomeBar.exe [switch]\n\n\
                     (none)\tStart HomeBar\n\
                     --quit\tClose the running HomeBar\n\
                     --keyboard\tToggle the on-screen keyboard\n\
                     --keyboard-visible\tExit code 1 if the keyboard is showing\n\
                     --screenshot\tSave a screenshot\n\
                     --show-desktop\tShow the desktop / bring windows back\n\
                     --check-updates [owner/repo]\tCheck GitHub for a newer release";

static STOP_PAD: AtomicBool = AtomicBool::new(false);

fn main() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hr = CoInitializeEx(null(), COINIT_APARTMENTTHREADED as _);
        if hr < 0 {
            util::log(&format!("COM unavailable ({hr:#010x}); audio, keyboard and screenshots won't work"));
        }
    }
    if !run_command_line() && single_instance() {
        run();
    }
    unsafe { CoUninitialize() };
}

fn run() {
    let Some(xinput) = XInput::load() else {
        message_box("Couldn't load xinput1_4.dll, so the controller can't be read.", MB_ICONERROR);
        return;
    };
    let Some(window) = create_windows() else {
        message_box("Couldn't create the menu window.", MB_ICONERROR);
        return;
    };
    audio::init();
    menu::refresh_audio(window);

    let pad = thread::spawn(move || pad::run(xinput, &STOP_PAD));
    thread::spawn(settings::offer_to_turn_off_game_bar_shortcut);

    unsafe {
        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    STOP_PAD.store(true, Relaxed);
    let _ = pad.join();
    audio::shut_down();
}

/// Switches for scripts and testing; returns true when the command line asked for one.
fn run_command_line() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => return false,
        Some("--quit") => unsafe {
            let running = FindWindowW(wide(MENU_CLASS).as_ptr(), null());
            if !running.is_null() {
                PostMessageW(running, WM_CLOSE, 0, 0);
            }
        },
        Some("--keyboard") => keyboard::toggle(),
        Some("--show-desktop") => apps::toggle_desktop(),
        Some("--keyboard-visible") => std::process::exit(keyboard::visible() as i32),
        Some("--screenshot") => {
            if let Err(why) = capture::save(cursor_monitor()) {
                util::log(&format!("screenshot failed: {why}"));
            }
        }
        Some("--check-updates") => {
            if let Some(url) = update::check(args.get(1).map_or(update::REPO, String::as_str)) {
                apps::open(&url);
            }
        }
        Some(_) => {
            message_box(USAGE, MB_ICONINFORMATION);
        }
    }
    true
}

fn cursor_monitor() -> RECT {
    unsafe {
        let mut at: POINT = zeroed();
        GetCursorPos(&mut at);
        let mut info: MONITORINFO = zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST), &mut info);
        info.rcMonitor
    }
}

fn single_instance() -> bool {
    unsafe {
        CreateMutexW(null(), 0, wide(r"Local\HomeBar").as_ptr());
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}

fn register_class(name: &[u16], proc: WNDPROC) -> bool {
    unsafe {
        let mut class: WNDCLASSW = zeroed();
        class.lpfnWndProc = proc;
        class.hInstance = GetModuleHandleW(null());
        class.lpszClassName = name.as_ptr();
        class.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
        RegisterClassW(&class) != 0
    }
}

fn set_dwm_flag(window: HWND, attribute: DWMWINDOWATTRIBUTE, value: u32) {
    unsafe { DwmSetWindowAttribute(window, attribute as u32, &value as *const u32 as *const c_void, size_of::<u32>() as u32) };
}

/// The hidden topmost menu window (also receives pad, tray and audio messages) and the tray menu's owner.
fn create_windows() -> Option<HWND> {
    let menu_class = wide(MENU_CLASS);
    let owner_class = wide(TRAY_OWNER_CLASS);
    if !register_class(&menu_class, Some(window_proc)) || !register_class(&owner_class, Some(DefWindowProcW)) {
        return None;
    }
    unsafe {
        let instance = GetModuleHandleW(null());
        let window = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            menu_class.as_ptr(),
            wide(util::APP_NAME).as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if window.is_null() {
            return None;
        }
        set_dwm_flag(window, DWMWA_USE_IMMERSIVE_DARK_MODE, 1);
        set_dwm_flag(window, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND as u32);
        // hides instantly, so a screenshot taken right after closing the menu doesn't catch it fading
        set_dwm_flag(window, DWMWA_TRANSITIONS_FORCEDISABLED, 1);
        state::set_menu_window(window);

        let owner = CreateWindowExW(WS_EX_TOOLWINDOW, owner_class.as_ptr(), null(), WS_POPUP, 0, 0, 0, 0, null_mut(), null_mut(), instance, null());
        tray::init(window, owner);
        Some(window)
    }
}

/// WM_SETTINGCHANGE names "ImmersiveColorSet" when the light/dark theme changes.
fn is_theme_change(l: LPARAM) -> bool {
    let area = l as *const u16;
    // stops at the first difference, so it never reads past a shorter string's terminator
    !area.is_null() && "ImmersiveColorSet".encode_utf16().chain(Some(0)).enumerate().all(|(i, c)| unsafe { *area.add(i) } == c)
}

unsafe extern "system" fn window_proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_PAD => {
            if let Some(event) = PadEvent::from_wparam(w) {
                menu::handle(h, event);
            }
        }
        tray::WM_TRAY => tray::on_message(h, l),
        audio::WM_AUDIO_CHANGED => menu::refresh_audio(h),
        audio::WM_AUDIO_DEVICES => {
            audio::reopen();
            menu::refresh_audio(h);
        }
        update::WM_OPEN_URL => update::open_posted_url(l),
        WM_ACTIVATE if (w & 0xffff) as u32 == WA_INACTIVE => menu::on_deactivate(h),
        WM_TIMER if w == tray::TIMER_PROMOTE => tray::on_promote_timer(h),
        WM_TIMER if menu::TIMERS.contains(&w) => menu::on_timer(h, w),
        WM_SETTINGCHANGE if is_theme_change(l) => tray::on_theme_changed(h),
        WM_PAINT => menu::paint(h),
        WM_ERASEBKGND => return 1,
        WM_DESTROY => {
            tray::remove_icons(h);
            PostQuitMessage(0);
        }
        _ if tray::is_taskbar_created(msg) => tray::add_icons(h),
        _ => return DefWindowProcW(h, msg, w, l),
    }
    0
}
