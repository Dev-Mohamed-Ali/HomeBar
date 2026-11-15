//! Taskbar icons: a gamepad while running (right-click menu) and a red mic while muted.

use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::util::{log, rgb, wide, APP_NAME};
use crate::{audio, settings, startup, update};

pub const WM_TRAY: u32 = WM_APP + 2;
pub const TIMER_PROMOTE: usize = 10;

const MAIN_ICON: u32 = 1;
const MIC_ICON: u32 = 2;
const CMD_UPDATE: usize = 1;
const CMD_EXIT: usize = 2;
const CMD_STARTUP: usize = 3;
const NIN_KEYSELECT: u32 = NIN_SELECT | 1; // NINF_KEY; missing from windows-sys

// Explorer records new icons in NotifyIconSettings a moment after NIM_ADD and says nothing when it does.
const PROMOTE_INTERVAL_MS: u32 = 500;
const PROMOTE_TRIES: u8 = 20;

const GLYPH_GAMEPAD: &str = "\u{E7FC}";
const GLYPH_MIC_OFF: &str = "\u{EC54}";

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

struct Tray {
    /// Owns the right-click menu. The menu window can't take focus, and a popup whose
    /// owner isn't foreground opens behind the taskbar.
    owner: HWND,
    main_icon: HICON,
    mic_icon: HICON,
    mic_shown: bool,
    /// Icons not yet in NotifyIconSettings, and timer ticks left to wait for them.
    promote_pending: Vec<u32>,
    promote_tries_left: u8,
    /// Opened when the "Screenshot saved" notification is clicked.
    last_screenshot: Option<std::path::PathBuf>,
}

thread_local! {
    static TRAY: RefCell<Tray> = const { RefCell::new(Tray {
        owner: null_mut(), main_icon: null_mut(), mic_icon: null_mut(), mic_shown: false, promote_pending: Vec::new(), promote_tries_left: 0, last_screenshot: None,
    }) };
}

fn with<R>(f: impl FnOnce(&mut Tray) -> R) -> R {
    TRAY.with(|t| f(&mut t.borrow_mut()))
}

pub fn init(h: HWND, owner: HWND) {
    with(|t| t.owner = owner);
    build_icons();
    TASKBAR_CREATED.store(unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) }, Relaxed);
    add_icons(h);
}

/// (Re)draws both icons; the gamepad matches the taskbar's light or dark theme.
fn build_icons() {
    let shade = if settings::taskbar_is_light() { 0x20 } else { 0xf2 };
    let main_icon = glyph_icon(GLYPH_GAMEPAD, rgb(shade, shade, shade));
    let mic_icon = glyph_icon(GLYPH_MIC_OFF, rgb(0xe8, 0x4a, 0x4a));
    let old = with(|t| [std::mem::replace(&mut t.main_icon, main_icon), std::mem::replace(&mut t.mic_icon, mic_icon)]);
    destroy(old);
}

fn destroy(icons: [HICON; 2]) {
    for icon in icons.into_iter().filter(|i| !i.is_null()) {
        unsafe { DestroyIcon(icon) };
    }
}

/// Redraws the icons in place after the taskbar switches between light and dark.
pub fn on_theme_changed(h: HWND) {
    build_icons();
    let (main_icon, mic_icon, mic_shown) = with(|t| (t.main_icon, t.mic_icon, t.mic_shown));
    set_icon(h, MAIN_ICON, main_icon);
    if mic_shown {
        set_icon(h, MIC_ICON, mic_icon);
    }
}

pub fn is_taskbar_created(msg: u32) -> bool {
    msg != 0 && msg == TASKBAR_CREATED.load(Relaxed)
}

/// Adds both icons, e.g. after Explorer restarts and the taskbar forgets them.
pub fn add_icons(h: HWND) {
    let icon = with(|t| {
        t.mic_shown = false;
        t.main_icon
    });
    if notify_icon(h, MAIN_ICON, Some((icon, &format!("{APP_NAME}: press Home on your controller")))) {
        start_promoting(h, MAIN_ICON);
    }
    update_mic_icon(h);
}

pub fn remove_icons(h: HWND) {
    notify_icon(h, MAIN_ICON, None);
    if with(|t| std::mem::take(&mut t.mic_shown)) {
        notify_icon(h, MIC_ICON, None);
    }
    unsafe { KillTimer(h, TIMER_PROMOTE) };
    let icons = with(|t| [std::mem::replace(&mut t.main_icon, null_mut()), std::mem::replace(&mut t.mic_icon, null_mut())]);
    destroy(icons);
}

fn start_promoting(h: HWND, id: u32) {
    with(|t| {
        if !t.promote_pending.contains(&id) {
            t.promote_pending.push(id);
        }
        t.promote_tries_left = PROMOTE_TRIES;
    });
    unsafe { SetTimer(h, TIMER_PROMOTE, PROMOTE_INTERVAL_MS, None) };
}

/// Checks until Explorer has recorded every pending icon, then stops.
pub fn on_promote_timer(h: HWND) {
    let recorded = settings::show_tray_icons();
    let done = with(|t| {
        t.promote_pending.retain(|id| !recorded.contains(id));
        t.promote_tries_left = t.promote_tries_left.saturating_sub(1);
        t.promote_pending.is_empty() || t.promote_tries_left == 0
    });
    if done {
        with(|t| t.promote_pending.clear());
        unsafe { KillTimer(h, TIMER_PROMOTE) };
    }
}

/// Shows the red mic only while the default mic is muted, whatever muted it.
pub fn update_mic_icon(h: HWND) {
    let muted = audio::mic() == audio::Mic::Muted;
    let (shown, icon) = with(|t| (t.mic_shown, t.mic_icon));
    if muted == shown {
        return;
    }
    let changed = notify_icon(h, MIC_ICON, muted.then_some((icon, "Microphone muted: click to unmute")));
    if changed {
        with(|t| t.mic_shown = muted);
        if muted {
            start_promoting(h, MIC_ICON);
        }
    }
}

pub fn on_message(h: HWND, l: LPARAM) {
    let event = (l & 0xffff) as u32;
    let id = ((l >> 16) & 0xffff) as u32;
    match (id, event) {
        (MAIN_ICON, WM_CONTEXTMENU | WM_RBUTTONUP) => context_menu(h),
        (MAIN_ICON, NIN_BALLOONUSERCLICK) => {
            if let Some(path) = with(|t| t.last_screenshot.clone()) {
                crate::apps::open(&path.to_string_lossy());
            }
        }
        (MIC_ICON, NIN_SELECT | NIN_KEYSELECT) => audio::toggle_mic(),
        _ => {}
    }
}

/// A notification from the gamepad icon; clicking it opens `open_on_click` if given.
pub fn notify(h: HWND, title: &str, text: &str, open_on_click: Option<std::path::PathBuf>) {
    with(|t| t.last_screenshot = open_on_click);
    unsafe {
        let mut data: NOTIFYICONDATAW = zeroed();
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = h;
        data.uID = MAIN_ICON;
        data.uFlags = NIF_INFO;
        data.dwInfoFlags = NIIF_INFO;
        for (dst, src) in data.szInfoTitle.iter_mut().zip(title.encode_utf16().take(63)) {
            *dst = src;
        }
        for (dst, src) in data.szInfo.iter_mut().zip(text.encode_utf16().take(255)) {
            *dst = src;
        }
        if Shell_NotifyIconW(NIM_MODIFY, &data) == 0 {
            log("tray: showing a notification failed");
        }
    }
}

fn set_icon(h: HWND, id: u32, icon: HICON) {
    unsafe {
        let mut data: NOTIFYICONDATAW = zeroed();
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = h;
        data.uID = id;
        data.uFlags = NIF_ICON;
        data.hIcon = icon;
        if Shell_NotifyIconW(NIM_MODIFY, &data) == 0 {
            log(&format!("tray: updating icon {id} failed"));
        }
    }
}

/// NIM_ADD with `Some((icon, tip))`, NIM_DELETE with `None`. False if the taskbar refused it.
fn notify_icon(h: HWND, id: u32, add: Option<(HICON, &str)>) -> bool {
    unsafe {
        let mut data: NOTIFYICONDATAW = zeroed();
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = h;
        data.uID = id;
        let Some((icon, tip)) = add else {
            return Shell_NotifyIconW(NIM_DELETE, &data) != 0;
        };
        data.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE | NIF_SHOWTIP;
        data.uCallbackMessage = WM_TRAY;
        data.hIcon = icon;
        for (dst, src) in data.szTip.iter_mut().zip(tip.encode_utf16().take(127)) {
            *dst = src;
        }
        if Shell_NotifyIconW(NIM_ADD, &data) == 0 {
            // normal while Explorer is starting; TaskbarCreated adds the icons again
            log(&format!("tray: adding icon {id} failed"));
            return false;
        }
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        Shell_NotifyIconW(NIM_SETVERSION, &data);
        true
    }
}

fn context_menu(h: HWND) {
    let owner = with(|t| t.owner);
    if owner.is_null() {
        return;
    }
    unsafe {
        let popup = CreatePopupMenu();
        let version = format!("{APP_NAME} {}", env!("CARGO_PKG_VERSION"));
        AppendMenuW(popup, MF_STRING | MF_GRAYED, 0, wide(&version).as_ptr());
        AppendMenuW(popup, MF_SEPARATOR, 0, null());
        let startup_check = if startup::is_enabled() { MF_CHECKED } else { MF_UNCHECKED };
        AppendMenuW(popup, MF_STRING | startup_check, CMD_STARTUP, wide("Start with Windows").as_ptr());
        AppendMenuW(popup, MF_STRING, CMD_UPDATE, wide("Check for updates").as_ptr());
        AppendMenuW(popup, MF_STRING, CMD_EXIT, wide(&format!("Exit {APP_NAME}")).as_ptr());

        let mut at: POINT = zeroed();
        GetCursorPos(&mut at);
        let mut monitor: MONITORINFO = zeroed();
        monitor.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST), &mut monitor);
        at.y = at.y.min(monitor.rcWork.bottom); // keep it above the taskbar

        SetForegroundWindow(owner);
        let flags = TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_RIGHTALIGN;
        let command = TrackPopupMenu(popup, flags, at.x, at.y, 0, owner, null()) as usize;
        DestroyMenu(popup);
        // documented TrackPopupMenu quirk: without it the next popup may not close on an outside click
        PostMessageW(owner, WM_NULL, 0, 0);

        match command {
            CMD_STARTUP => {
                startup::set_enabled(!startup::is_enabled());
            }
            CMD_UPDATE => update::check_in_background(),
            CMD_EXIT => {
                DestroyWindow(h);
            }
            _ => {}
        }
    }
}

/// Renders a Segoe MDL2 Assets glyph into a small alpha icon.
fn glyph_icon(glyph: &str, color: u32) -> HICON {
    let (r, g, b) = (color & 0xff, (color >> 8) & 0xff, (color >> 16) & 0xff);
    unsafe {
        // tray icons follow the system DPI, which is fixed for the life of the process
        let size = GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()).max(16);
        let mut info: BITMAPINFO = zeroed();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = size;
        info.bmiHeader.biHeight = -size; // top-down
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;

        let screen = GetDC(null_mut());
        let mut bits: *mut c_void = null_mut();
        let dib = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        let dc = CreateCompatibleDC(screen);
        ReleaseDC(null_mut(), screen);
        if dib.is_null() || bits.is_null() {
            DeleteDC(dc);
            return null_mut();
        }
        let old_bitmap = SelectObject(dc, dib);

        // white on black, so each pixel's brightness becomes its alpha
        let font = CreateFontW(-(size * 15 / 16), 0, 0, 0, 400, 0, 0, 0, 0, 0, 0, ANTIALIASED_QUALITY as _, 0, wide("Segoe MDL2 Assets").as_ptr());
        let old_font = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as _);
        SetTextColor(dc, rgb(255, 255, 255));
        let mut area = RECT { left: 0, top: 0, right: size, bottom: size };
        DrawTextW(dc, wide(glyph).as_ptr(), -1, &mut area, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        GdiFlush();

        let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, (size * size * 4) as usize);
        for p in pixels.as_chunks_mut::<4>().0 {
            let alpha = p[0].max(p[1]).max(p[2]) as u32;
            // premultiplied BGRA
            p[0] = (b * alpha / 255) as u8;
            p[1] = (g * alpha / 255) as u8;
            p[2] = (r * alpha / 255) as u8;
            p[3] = alpha as u8;
        }

        SelectObject(dc, old_font);
        DeleteObject(font);
        SelectObject(dc, old_bitmap);
        DeleteDC(dc);

        let mask = CreateBitmap(size, size, 1, 1, null());
        let icon_info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: dib };
        let icon = CreateIconIndirect(&icon_info);
        DeleteObject(mask);
        DeleteObject(dib);
        icon
    }
}
